# MIDL to Rust Mapping

The Rust backend generates plain Rust modules. It targets the current
`motherboard_client` API and uses `postcard` for payload encoding.

## Module Shape

Each service becomes a snake-case module:

```text
service Auth { ... }
```

generates:

```rust
pub mod auth {
    pub const SERVICE: &str = "Auth";
}
```

Generated items live inside that module to avoid collisions between services.

## Scalar Types

MIDL primitive mappings:

```text
u8..u64     -> u8..u64
i8..i64     -> i8..i64
string      -> String
bool        -> bool
char        -> char
f32/f64     -> f32/f64
void        -> ()
T[]         -> Vec<T>
(A, B)      -> (A, B)
AnonymousStore<T> -> AnonymousStore<T>
```

`AnonymousStore<T>` is a generated typed wrapper around
`motherboard_client::AnonymousStoreId`:

```rust
pub struct AnonymousStore<T> {
    id: motherboard_client::AnonymousStoreId,
    _marker: core::marker::PhantomData<fn() -> T>,
}
```

The wrapper keeps Rust type information while preserving the protocol payload as
the opaque id.

## Serialization

The generated code derives `serde::Serialize` and `serde::Deserialize` for
payload types. Requests and replies are encoded with `postcard`.

Function arguments are encoded as a tuple in declaration order. A zero-argument
function uses `()`.

```text
fn login_with_password(sessionId: AuthSessionToken, password: string) -> bool;
```

uses:

```rust
(session_id, password)
```

as the request payload.

Successful replies encode the declared return type. `void` encodes `()`.

## Names

MIDL service and enum names are emitted as written. Function, parameter, field,
store, and type alias names are converted only when needed for Rust keywords in
future versions; the prototype currently emits them as written.

Function method names sent to the kernel are the exact MIDL names.

## Type Aliases

```text
type AuthSessionToken = string;
```

generates:

```rust
pub type AuthSessionToken = String;
```

## Enums

MIDL enums generate Rust enums with serde derives:

```text
enum FarProcessState {
    Running,
    Crashed { logs: string }
}
```

generates:

```rust
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum FarProcessState {
    Running,
    Crashed { logs: String },
}
```

## Stores

Named stores generate typed client handles. Store payloads are still serialized
with `postcard`, but application code does not pass service or store names as
strings.

```text
store namedValue: string;
```

generates client-side types like:

```rust
pub struct NamedValueStore;
pub struct NamedValueStoreSubscription {
    pub id: motherboard_client::SubscriptionId,
}
```

Usage:

```rust
let sub = NamedValueStore::subscribe().await?;
match NamedValueStore::next_event(&sub).await? {
    StoreEvent::Accepted { value, .. } => { /* value: String */ }
    StoreEvent::Updated(value) => { /* value: String */ }
    StoreEvent::Rejected { message } => { /* ... */ }
    StoreEvent::Closed { reason } => { /* ... */ }
}
```

If a store is not declared in the IDL, its Rust handle type is not generated and
client code cannot compile against it.

Private stores require handling `InboxMessage::SubscribeRequest` and replying
with `accept_subscription` or `reject_subscription`.

Anonymous store values are created by the server with:

```rust
server.stores().create_anonymous(SERVICE, postcard::to_allocvec(&value)?)
```

and returned from functions as `AnonymousStore<T>`.

## Rust Generation Modes

The Rust backend has two explicit sides:

```text
midl auth.midl --mode client --rust-out auth_client.rs
midl auth.midl --mode server --rust-out auth_server.rs
midl auth.midl --mode both   --rust-out auth.rs
```

`both` is the default while the prototype is still small.

The client side is optimized for application code. It exposes typed global
async service functions that submit the motherboard request, wait for the
matching reply, decode the payload, and return the declared Rust type. The
generated code owns one lazy global motherboard connection per generated service
module; applications do not need to create or store a client instance.

The server side is optimized for service implementations. It emits a service
trait, a context type for store operations, and a runtime that installs one
implementation of that trait and dispatches requests concurrently.

## Generated Client Wrappers

For each service, the backend emits a typed service namespace:

```rust
pub struct AuthService;
```

Each function becomes a typed async helper:

```rust
pub async fn start_user_auth(
    user: String,
) -> BindingResult<Result<AuthSessionToken, StartUserAuthError>>
```

That allows application code to use the service directly:

```rust
let session = auth::AuthService::start_user_auth("tiago".to_string()).await??;
```

Because motherboard inboxes can contain function replies, store updates,
subscription decisions, and service-close messages in the same stream, generated
clients must never discard messages while waiting for one request id. The client
wrapper includes a small inbox router:

- replies are cached by `RequestId`;
- non-reply messages are stored in an application-visible queue;
- `AuthService::method(...).await` waits only for its matching reply;
- `AuthService::next_message().await` drains non-RPC messages without losing replies.

## Generated Server Wrappers

For each service, the server backend emits a trait implemented by service code:

```rust
#[async_trait::async_trait]
pub trait AuthService: Send + Sync + 'static {
    async fn initial_totalComponents(&self) -> BindingResult<Vec<String>>;

    async fn auth_session_status(
        &self,
        ctx: &mut AuthContext<'_>,
        origin: motherboard_client::Origin,
        sessionId: AuthSessionToken,
    ) -> Result<AnonymousStore<AuthSessionStatus>, AuthSessionStatusError>;

    async fn authorize_auth_session_status_subscription(
        &self,
        origin: motherboard_client::Origin,
        store: AnonymousStore<AuthSessionStatus>,
    ) -> bool;
}
```

The context type exposes generated helpers such as
`create_auth_session_status_store`, `update_auth_session_status_store`, and
named store update helpers. Initial values for stores declared in the IDL come
from required `initial_<store>` methods on the trait. The generated runtime
calls those initializers when the service implementation is installed, then
creates the stores automatically.

The generated `ServerRuntime` is shared by all services in one generated binding
set. A server process can install multiple service implementations into the same
runtime and they all share one motherboard connection. The runtime binds each
installed service, decodes requests, calls the matching trait implementation,
and sends the appropriate success or error replies.

Typical server code looks like:

```rust
let mut runtime = ServerRuntime::new()?;
runtime.install_auth(AuthImpl::new()).await?;
runtime.install_init(InitImpl::new()).await?;
runtime.serve().await?;
```

Function requests are intended to run concurrently. A server event loop should
fetch inbox messages sequentially only long enough to decode and dispatch them.
The generated runtime calls `tokio::spawn` for each accepted function request.
One slow request must not stop the service from starting later requests.

Subscription authorization messages may also be spawned by the service if the
authorization path can block or perform I/O.

## Error Sets

Inline MIDL errors generate one Rust enum per function:

```text
fn cancel_login(sessionId: AuthSessionToken) -> void?(error { NotFound });
```

generates:

```rust
pub enum CancelLoginError {
    NotFound,
}
```

Typed service errors are part of the function payload, not the transport status.
For a function declared with an error set, the wire reply payload is:

```rust
Result<ReturnType, FunctionErrorEnum>
```

and it is sent with `ReplyStatus::Ok`. The generated client deserializes that
payload as `Result<T, E>`, so service errors remain typed end-to-end.

`ReplyStatus::Error` is reserved for transport/runtime failures where the typed
IDL function payload cannot be produced.

## Anonymous Store Authorization

The generated server side should make anonymous subscription authorization easy
to keep local to the store creator. The intended high-level helper shape is:

```rust
let store = auth.create_auth_session_status_store(
    initial_status,
    move |origin| authorize_using_captured_session(origin),
)?;
```

The closure-based helper captures the session id or process identity without
requiring service code to maintain a separate
`HashMap<AnonymousStoreId, SessionId>`. Generated server wrappers keep a small
internal registry from anonymous store id to authorizer closure. When the event
loop receives `InboxMessage::AnonymousStoreSubscribeRequest`, it calls the
generated `authorize_anonymous_subscription` helper, which invokes the stored
closure and replies with `accept_subscription` or `reject_subscription`.

The generated server also emits `update_<function>_store` for functions that
return `AnonymousStore<T>`, so the service can update the anonymous store by
typed wrapper instead of raw id.
