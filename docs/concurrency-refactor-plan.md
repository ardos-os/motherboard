# Concurrency Refactor Plan

`motherboardm` currently protects almost all transport state behind one global
`Mutex<Option<State>>`. That is good enough for early correctness, but it
serializes unrelated operations. For example, two clients fetching from two
different inboxes do not touch the same queue, but they still wait on the same
global lock.

This note captures the intended refactor so it can be implemented after the
userspace client library is updated for the new protocol names.

## Prerequisite

Update `client/src/lib.rs` and examples to use the current protocol:

- `Command::FunctionCall`
- `Command::FunctionCallReply`
- `Command::StoreSubscribe`
- `Command::StoreSubscriptionReply`
- `Command::StoreCreate`
- `Command::StoreUpdate`
- `Command::StoreUnsubscribe`
- `Command::InboxNextMessage`
- `CommandReply::FunctionCallAccepted`
- `CommandReply::FunctionCallReplyAccepted`
- `CommandReply::StoreSubscriptionAccepted`
- `CommandReply::StoreSubscriptionReplyAccepted`
- `CommandReply::StoreCreateAccepted`
- `CommandReply::StoreUpdateAccepted`
- `CommandReply::StoreUnsubscribed`
- `CommandReply::InboxMessagePopped`

The client still expects the older names such as `Call`, `Reply`, `Fetch`,
`Submitted`, and `Message`, so it must be updated before runtime testing can
cover the store and subscription paths.

## Goal

Replace the single coarse global mutable lock with a layout where unrelated
operations can proceed independently:

- fetching from inbox A should not block fetching from inbox B;
- polling a latch should not require global mutable state;
- reading service/store registries should not block other readers;
- token consumption should only lock the token table;
- store updates should only hold registry locks long enough to compute the
  affected subscribers and message payloads.

## Proposed State Shape

Move from:

```rust
static GLOBAL_STATE: Mutex<Option<State>>;

pub struct State {
    services: HashMap<SharedStr, Service>,
    inboxes: Inboxes,
    reply_tokens: ReplyTokens,
    stores: StoreMap,
    subscriptions: Subscriptions,
    store_subscription_reply_tokens: StoreSubscriptionReplyTokens,
}
```

Toward:

```rust
static GLOBAL_STATE: RwLock<Option<State>>;

pub struct State {
    services: RwLock<ServiceRegistry>,
    inboxes: Inboxes,
    stores: RwLock<StoreMap>,
    subscriptions: RwLock<Subscriptions>,
    reply_tokens: Mutex<ReplyTokens>,
    store_subscription_reply_tokens: Mutex<StoreSubscriptionReplyTokens>,
}
```

`RefCell` should not be used for shared kernel state. This code is reachable
concurrently from different tasks, so use kernel-safe locks, atomics, or
per-object synchronization instead.

## Inbox Refactor

Make `Inboxes` internally concurrent.

Target shape:

```rust
pub struct Inboxes {
    map: RwLock<HashMap<FileId, Arc<Inbox>>>,
}

pub struct Inbox {
    messages: Mutex<VecDeque<Message>>,
    generation: AtomicU64,
    wait_queue: PollCondVar,
}
```

Expected behavior:

- `InboxNextMessage` locks only the caller's inbox queue.
- Queueing a message locks only the destination inbox queue.
- `poll_latch` reads `generation` atomically, registers on that inbox wait
  queue, then rechecks `generation`.
- `generation` increments whenever a message is queued or the inbox is otherwise
  notified.

This removes pointless serialization between unrelated inboxes.

## Command Locking Model

Suggested lock scope per command:

- `BindService`: write lock `services`; create or ensure the provider inbox.
- `FunctionCall`: read lock `services` to resolve the service `FileId`; capture
  sender fds; queue to the service request queue or directly to the service
  inbox.
- `FunctionCallReply`: lock `reply_tokens`; consume token; queue reply into the
  original client inbox.
- `InboxNextMessage`: lock only the caller inbox. If service request queues
  remain separate from inboxes, lock only that one service queue.
- `StoreCreate`: read/check service ownership; write lock `stores`.
- `StoreSubscribe`: read `stores`; write lock `subscriptions`; queue either a
  client acceptance message or a server approval request.
- `StoreSubscriptionReply`: lock store-subscription reply tokens; then update
  `subscriptions` and queue the result to the client.
- `StoreUpdate`: write/update the one store entry; read subscriptions for that
  store; drop registry locks; queue one `StoreSubscriptionUpdated` per
  subscriber.
- `StoreUnsubscribe`: write lock `subscriptions`.
- `cleanup`: remove service/client-owned state under short locks, gather closure
  notifications, then queue inbox messages after dropping registry locks.

## Important Rule

Do not hold a registry lock while doing potentially expensive or unrelated work.

In particular, avoid holding `services`, `stores`, or `subscriptions` locks
while:

- installing or cloning file descriptors;
- queueing many inbox messages;
- serializing data;
- doing user-copy-adjacent work;
- allocating large payload structures.

Instead:

1. take the smallest lock needed;
2. copy or clone the target `FileId`s, `StorePath`s, reply tokens, and payload
   snapshots;
3. drop the lock;
4. perform fd work and queue messages.

## Migration Steps

1. Update the userspace client crate and examples to the new protocol names.
2. Add tests or examples that cover:
   - function call/reply;
   - fd passing;
   - public store subscribe/update;
   - private store subscribe/reply/update;
   - unsubscribe;
   - service cleanup notification.
3. Refactor `Inboxes` to use per-inbox locking and atomic latch generations.
4. Move token managers behind their own `Mutex`.
5. Move service and store registries behind `RwLock`.
6. Shorten command critical sections so locks are not held while queueing fanout
   notifications.
7. Re-run `cargo nok build` after each step and runtime-test with the example
   server/client.

## Expected Result

The common hot paths become independent:

- two unrelated clients can fetch inbox messages concurrently;
- a latch poll does not block command execution globally;
- store readers do not block each other;
- token consumption does not block store registry access;
- fanout work happens outside registry locks.

The design remains simple enough to audit: registries use `RwLock`, mutable
single-purpose managers use `Mutex`, inbox queues use per-inbox `Mutex`, and
latch readiness uses atomics plus `PollCondVar`.
