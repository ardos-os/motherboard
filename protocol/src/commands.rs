use alloc::boxed::Box;
use serde::{Deserialize, Serialize};

/// Owned str stored on the heap.
pub type Str = Box<str>;
/// Owned array stored on the heap.
pub type Array<T> = Box<[T]>;
/// Owned array of bytes stored on the heap.
pub type Data = Array<u8>;
/// A userspace file descriptor number.
pub type RawFd = u32;

/// Caller-selected request identifier, unique within one motherboard connection.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct RequestId(pub u64);

/// Caller-selected subscription identifier, unique within one motherboard connection.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct SubscriptionId(pub u64);

/// Kernel-issued token that lets a service reply to exactly one pending request.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct ReplyToken(pub u64);

/// Identity metadata attested by the kernel, never supplied by userspace.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Origin {
    pub pid: u32,
    pub uid: u32,
    pub gid: u32,
    pub is_trusted: bool,
}

/// Operations submitted by userspace to motherboardm.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// Binds the current open motherboard connection as the owner of a service name.
    BindService { name: Str },

    /// Calls an opaque service method asynchronously.
    Call {
        service: Str,
        method: Str,
        request_id: RequestId,
        payload: Data,
        fds: Array<RawFd>,
    },

    /// Replies to a service call or subscription request using a kernel-issued token.
    Reply {
        reply_token: ReplyToken,
        status: ReplyStatus,
        payload: Data,
        fds: Array<RawFd>,
    },

    /// Requests a state/event subscription from a service.
    Subscribe {
        service: Str,
        store: Str,
        subscription_id: SubscriptionId,
        payload: Data,
    },

    /// Accepts or rejects a pending subscription request.
    SubscriptionReply {
        reply_token: ReplyToken,
        accepted: bool,
        payload: Data,
    },

    /// Emits an update to an accepted subscription.
    UpdateStore {
        store: Str,
        payload: Data,
        fds: Array<RawFd>,
    },

    /// Cancels a previously requested or accepted subscription.
    Cancel { subscription_id: SubscriptionId },

    /// Fetches the next asynchronous message for the current open connection.
    Fetch,
}

/// Immediate kernel response to an ioctl command.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CommandReply {
    ServiceBound,
    Submitted { request_id: RequestId },
    SubscriptionSubmitted { subscription_id: SubscriptionId },
    Replied,
    SubscriptionReplied,
    Emitted,
    Cancelled,
    Message(InboxMessage),
}

/// Messages delivered asynchronously through a connection inbox.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum InboxMessage {
    CallRequest {
        service: Str,
        method: Str,
        request_id: RequestId,
        reply_token: ReplyToken,
        origin: Origin,
        payload: Data,
        fds: Array<RawFd>,
    },
    CallReply {
        request_id: RequestId,
        status: ReplyStatus,
        payload: Data,
        fds: Array<RawFd>,
    },
    SubscribeRequest {
        service: Str,
        topic: Str,
        subscription_id: SubscriptionId,
        reply_token: ReplyToken,
        origin: Origin,
        payload: Data,
    },
    SubscriptionAccepted {
        subscription_id: SubscriptionId,
        payload: Data,
    },
    SubscriptionRejected {
        subscription_id: SubscriptionId,
        payload: Data,
    },
    SubscriptionEvent {
        subscription_id: SubscriptionId,
        payload: Data,
        fds: Array<RawFd>,
    },
    SubscriptionClosed {
        subscription_id: SubscriptionId,
        reason: CloseReason,
    },
    ServiceClosed {
        service: Str,
        reason: CloseReason,
    },
}

/// Service-level status for asynchronous replies.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ReplyStatus {
    Ok,
    Error { code: Str, message: Str },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CloseReason {
    Closed,
    ServiceExited,
    Cancelled,
    ProtocolError,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, thiserror::Error)]
pub enum TransportError {
    #[error("Command is not allowed for the current PID, UID, GID, namespace, or security context")]
    Unauthorized,
    #[error("Failed to bind service because a service with the same name already exists")]
    ServiceNameConflict,
    #[error("Service name is invalid")]
    InvalidServiceName { message: Str },
    #[error("Service does not exist")]
    NoSuchService,
    #[error("An attached userspace file descriptor number is invalid")]
    InvalidFileDescriptor,
    #[error("The kernel could not allocate a required transport resource")]
    ResourceExhausted,
    #[error("Reply token is invalid, expired, already consumed, or owned by another connection")]
    InvalidReplyToken,
    #[error("Subscription does not exist")]
    NoSuchSubscription,
    #[error("The inbox is empty; poll the returned latch fd before fetching again")]
    WouldBlock { latch_fd: RawFd },
    #[error("Command is recognized but is not implemented yet")]
    NotImplemented,
}
