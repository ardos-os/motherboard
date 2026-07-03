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
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum StoreSubscriptionServerVerdict {
    Accepted,
    Rejected { message: Str },
}

/// Operations submitted by userspace to motherboardm.
///
/// These commands are encoded by `postcard` and `serde`, the encoded command buffer then is sent to `motherboardm` through ioctl.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum Command {
    /// Binds the current open motherboard connection as the owner of a service name. (Server side)
    BindService { name: Str },

    /// Calls an opaque service method asynchronously. (Client Side)
    FunctionCall {
        service: Str,
        method: Str,
        request_id: RequestId,
        payload: Data,
        fds: Array<RawFd>,
    },

    /// Replies to a service call or subscription request using a kernel-issued token. (Server side)
    FunctionCallReply {
        reply_token: ReplyToken,
        status: ReplyStatus,
        payload: Data,
        fds: Array<RawFd>,
    },

    /// Requests a store subscription from a service. (Client Side)
    StoreSubscribe {
        service: Str,
        store: Str,
        subscription_id: SubscriptionId,
        payload: Data,
    },

    /// Accepts or rejects a pending subscription request. (Server Side)
    StoreSubscriptionReply {
        reply_token: ReplyToken,
        verdict: StoreSubscriptionServerVerdict,
    },

    /// Creates a new store inside a service with an initial value. (Server Side)
    ///
    /// the `public` flag tells the module whenever it needs to ask the server for permission to allow subscriptions or if it can immediately subscribe without asking anything.
    StoreCreate {
        service: Str,
        store: Str,
        initial_value: Data,
        public: bool,
    },
    /// Updates the value of a store, notifying all listeners about the change. (Server Side)
    StoreUpdate {
        service: Str,
        store: Str,
        value: Data,
    },

    /// Cancels a previously requested or accepted subscription. (Client Side)
    StoreUnsubscribe { subscription_id: SubscriptionId },

    /// Fetches the next asynchronous message for the current open connection. (Client and Server Side)
    InboxNextMessage,
}

/// Immediate kernel response to an ioctl command.
///
/// `CommandReply` is received encoded from a file descriptor returned by the `ioctl` call which is then read until EOF and decoded with `postcard` into this type.
///
/// # Name conventions
///
/// There are some name conventions to the variants names for consistency and to know exactly what each thing means.
///
/// ## ...Accepted
///
/// Like http code `202 Accepted`, this means the request was received but not fully processed yet. It will be processed in the background asynchronously.
///
/// ## Store...
///
/// Store related command replies always start with `Store`
///
/// ## StoreSubscription...
///
///
/// Store subscription related commands always start with `StoreSubscription`
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CommandReply {
    // --------- RPC Module ----------
    /// Response to [`Command::BindService`]
    ServiceBound,
    /// Confirmation to [`Command::FunctionCall`]
    FunctionCallAccepted {
        request_id: RequestId,
    },
    FunctionCallReplyAccepted,
    // --------- Store Module ----------
    /// Confirmation to [`Command::StoreSubscribe`]
    StoreSubscriptionAccepted {
        subscription_id: SubscriptionId,
    },
    /// Confirmation to [`Command::StoreSubscriptionReply`]
    StoreSubscriptionReplyAccepted,
    /// Confirmation to [`Command::StoreCreate`]
    StoreCreateAccepted,
    /// Confirmation to [`Command::StoreUpdate`]
    StoreUpdateAccepted,
    /// Confirmation to [`Command::StoreUnsubscribe`]
    StoreUnsubscribed,

    // -----------   Inbox  ------------
    /// Result of calling [`Command::InboxNextMessage`] while there are messages in the inbox.
    InboxMessagePopped(InboxMessage),
}

/// Messages delivered asynchronously through a connection inbox.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum InboxMessage {
    // RPC
    /// A function call came from a client.
    FunctionCallRequest {
        service: Str,
        method: Str,
        request_id: RequestId,
        reply_token: ReplyToken,
        origin: Origin,
        payload: Data,
        fds: Array<RawFd>,
    },
    /// The server responded to a previous function call.
    FunctionCallReply {
        request_id: RequestId,
        status: ReplyStatus,
        payload: Data,
        fds: Array<RawFd>,
    },

    // Store
    /// A client is trying to subscribe to a store in this service.
    SubscribeRequest {
        service: Str,
        store: Str,
        subscription_id: SubscriptionId,
        reply_token: ReplyToken,
        origin: Origin,
        payload: Data,
    },

    /// The server accepted the subscription to a store
    StoreSubscriptionAccepted {
        service: Str,
        store: Str,
        subscription_id: SubscriptionId,
        /// The last value the module has retained from the last update the server sent
        current_value: Data,
        /// Monotonic timestamp assigned by the module when this value was stored.
        last_updated_timestamp: isize,
    },
    /// The server rejected the subscription
    StoreSubscriptionRejected {
        service: Str,
        store: Str,
        subscription_id: SubscriptionId,
        message: Str,
    },

    /// A store you were subscribed to just updated
    StoreSubscriptionUpdated {
        service: Str,
        store: Str,
        subscription_id: SubscriptionId,
        payload: Data,
    },

    /// Store subscription was terminated by the client or the server
    StoreSubscriptionClosed {
        service: Str,
        store: Str,
        subscription_id: SubscriptionId,
        reason: CloseReason,
    },

    /// The server behind the service closed unexpectedly and the service is no longer available
    ServiceClosed { service: Str, reason: CloseReason },
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
    #[error("Store does not exist")]
    NoSuchStore,
    #[error("Store already exists")]
    StoreAlreadyExists,
    #[error("Store name is invalid")]
    InvalidStoreName { message: Str },
    #[error("A store subscription with the same id already exists for this connection")]
    SubscriptionIdConflict,
    #[error("An attached userspace file descriptor number is invalid")]
    InvalidFileDescriptor,
    #[error("The kernel could not allocate a required transport resource")]
    ResourceExhausted,
    #[error("Reply token is invalid, expired, already consumed, or owned by another connection")]
    InvalidReplyToken,
    #[error("Store Subscription does not exist")]
    NoSuchSubscription,
    #[error("The inbox is empty; poll the returned latch fd before fetching again")]
    WouldBlock { latch_fd: RawFd },
    #[error("Command is recognized but is not implemented yet")]
    NotImplemented,
}
