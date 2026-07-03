use std::{
    error::Error,
    fmt,
    fs::{File, OpenOptions},
    io::{self, Read},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

pub use motherboardm_protocol::{
    CloseReason, Command, CommandReply, CommandResult, InboxMessage, Origin, RawFd, ReplyStatus,
    ReplyToken, RequestId, StoreSubscriptionServerVerdict, SubscriptionId, TransportError,
};
use motherboardm_protocol::{CommandEnvelope, MOTHERBOARD_IOCTL_EXECUTE};

const DEVICE_PATH: &str = "/dev/services";

#[derive(Debug)]
pub enum ClientError {
    Io(io::Error),
    Protocol(String),
    Transport(TransportError),
    WouldBlock(OwnedFd),
    UnexpectedReply(CommandReply),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "io error: {error}"),
            Self::Protocol(error) => write!(f, "protocol error: {error}"),
            Self::Transport(error) => write!(f, "transport error: {error}"),
            Self::WouldBlock(_) => write!(f, "operation would block"),
            Self::UnexpectedReply(reply) => write!(f, "unexpected command reply: {reply:?}"),
        }
    }
}

impl Error for ClientError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            Self::Transport(error) => Some(error),
            _ => None,
        }
    }
}

impl From<io::Error> for ClientError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<TransportError> for ClientError {
    fn from(error: TransportError) -> Self {
        Self::Transport(error)
    }
}

pub struct MotherboardClient {
    device: File,
    next_request_id: AtomicU64,
    next_subscription_id: AtomicU64,
}

impl MotherboardClient {
    /// Opens `/dev/services`.
    pub fn open() -> io::Result<Self> {
        Self::open_path(DEVICE_PATH)
    }

    /// Opens a motherboard device at a custom path.
    pub fn open_path(path: impl AsRef<Path>) -> io::Result<Self> {
        let device = OpenOptions::new().read(true).write(true).open(path)?;
        Ok(Self {
            device,
            next_request_id: AtomicU64::new(1),
            next_subscription_id: AtomicU64::new(1),
        })
    }

    pub fn next_request_id(&self) -> RequestId {
        RequestId(self.next_request_id.fetch_add(1, Ordering::Relaxed))
    }

    pub fn next_subscription_id(&self) -> SubscriptionId {
        SubscriptionId(self.next_subscription_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Executes one command through `MOTHERBOARD_IOCTL_EXECUTE` and returns the raw protocol result.
    pub fn execute_raw(&self, command: Command) -> io::Result<CommandResult> {
        let cmd_buf = command
            .serialize_to_extendable(Vec::new())
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{error:?}")))?;

        let envelope = CommandEnvelope {
            version: motherboardm_protocol::ABI_VERSION,
            data_ptr: cmd_buf.as_ptr().addr(),
            data_len: cmd_buf.len(),
        };

        // SAFETY: The request number and envelope layout are shared with the kernel module, and
        // cmd_buf remains alive and immutable for the duration of the ioctl.
        let result_fd = unsafe {
            libc::ioctl(
                self.device.as_raw_fd(),
                MOTHERBOARD_IOCTL_EXECUTE as libc::c_ulong,
                &envelope,
            )
        };

        if result_fd < 0 {
            return Err(io::Error::last_os_error());
        }

        // SAFETY: A successful ioctl returns a new descriptor owned by this process.
        let result_fd = unsafe { OwnedFd::from_raw_fd(result_fd) };
        let mut result_file = File::from(result_fd);
        let mut result_bytes = Vec::new();
        result_file.read_to_end(&mut result_bytes)?;

        CommandResult::deserialize_from_bytes(&result_bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, format!("{error:?}")))
    }

    pub fn execute(&self, command: Command) -> Result<CommandReply, ClientError> {
        let CommandResult(result) = self.execute_raw(command)?;
        match result {
            Ok(reply) => Ok(reply),
            Err(TransportError::WouldBlock { latch_fd }) => {
                // SAFETY: The kernel returned a non-negative fd in this process as part of the
                // command result, and ownership transfers to this client error.
                let latch_fd = unsafe { OwnedFd::from_raw_fd(latch_fd as _) };
                Err(ClientError::WouldBlock(latch_fd))
            }
            Err(error) => Err(ClientError::Transport(error)),
        }
    }

    pub fn client(&self) -> impl ClientApi + '_ {
        ClientNamespace { motherboard: self }
    }

    pub fn server(&self) -> impl ServerApi + '_ {
        ServerNamespace { motherboard: self }
    }

    #[cfg(feature = "tokio")]
    async fn fetch_async(&self) -> Result<InboxMessage, ClientError> {
        loop {
            match fetch(self) {
                Ok(message) => return Ok(message),
                Err(ClientError::WouldBlock(latch_fd)) => {
                    let latch = tokio::io::unix::AsyncFd::new(latch_fd)?;
                    {
                        let mut guard = latch.readable().await?;
                        guard.clear_ready();
                    }
                    drop(latch);
                }
                Err(error) => return Err(error),
            }
        }
    }
}

pub trait ClientApi {
    fn calls(&self) -> impl ClientCallsApi + '_;
    fn stores(&self) -> impl ClientStoresApi + '_;
    fn fetch(&self) -> Result<InboxMessage, ClientError>;

    #[cfg(feature = "tokio")]
    fn fetch_async(
        &self,
    ) -> impl core::future::Future<Output = Result<InboxMessage, ClientError>> + '_;
}

pub trait ServerApi {
    fn calls(&self) -> impl ServerCallsApi + '_;
    fn stores(&self) -> impl ServerStoresApi + '_;
    fn bind_service(&self, name: &str) -> Result<(), ClientError>;
    fn register_service(&self, name: &str) -> Result<(), ClientError>;
    fn fetch(&self) -> Result<InboxMessage, ClientError>;

    #[cfg(feature = "tokio")]
    fn fetch_async(
        &self,
    ) -> impl core::future::Future<Output = Result<InboxMessage, ClientError>> + '_;
}

pub trait ClientCallsApi {
    fn call(
        &self,
        service: &str,
        method: &str,
        payload: impl Into<Box<[u8]>>,
        fds: impl Into<Box<[RawFd]>>,
    ) -> Result<RequestId, ClientError>;

    fn call_with_id(
        &self,
        service: &str,
        method: &str,
        request_id: RequestId,
        payload: impl Into<Box<[u8]>>,
        fds: impl Into<Box<[RawFd]>>,
    ) -> Result<RequestId, ClientError>;
}

pub trait ServerCallsApi {
    fn reply(
        &self,
        reply_token: ReplyToken,
        status: ReplyStatus,
        payload: impl Into<Box<[u8]>>,
        fds: impl Into<Box<[RawFd]>>,
    ) -> Result<(), ClientError>;
}

pub trait ClientStoresApi {
    fn subscribe(
        &self,
        service: &str,
        store: &str,
        payload: impl Into<Box<[u8]>>,
    ) -> Result<SubscriptionId, ClientError>;

    fn subscribe_with_id(
        &self,
        service: &str,
        store: &str,
        subscription_id: SubscriptionId,
        payload: impl Into<Box<[u8]>>,
    ) -> Result<SubscriptionId, ClientError>;

    fn unsubscribe(&self, subscription_id: SubscriptionId) -> Result<(), ClientError>;
}

pub trait ServerStoresApi {
    fn create(
        &self,
        service: &str,
        store: &str,
        initial_value: impl Into<Box<[u8]>>,
        public: bool,
    ) -> Result<(), ClientError>;

    fn update(
        &self,
        service: &str,
        store: &str,
        value: impl Into<Box<[u8]>>,
    ) -> Result<(), ClientError>;

    fn subscription_reply(
        &self,
        reply_token: ReplyToken,
        verdict: StoreSubscriptionServerVerdict,
    ) -> Result<(), ClientError>;

    fn accept_subscription(&self, reply_token: ReplyToken) -> Result<(), ClientError>;

    fn reject_subscription(
        &self,
        reply_token: ReplyToken,
        message: impl Into<Box<str>>,
    ) -> Result<(), ClientError>;
}

pub struct ClientNamespace<'a> {
    motherboard: &'a MotherboardClient,
}

pub struct ServerNamespace<'a> {
    motherboard: &'a MotherboardClient,
}

pub struct ClientCallsNamespace<'a> {
    motherboard: &'a MotherboardClient,
}

pub struct ServerCallsNamespace<'a> {
    motherboard: &'a MotherboardClient,
}

pub struct ClientStoresNamespace<'a> {
    motherboard: &'a MotherboardClient,
}

pub struct ServerStoresNamespace<'a> {
    motherboard: &'a MotherboardClient,
}

impl ClientApi for ClientNamespace<'_> {
    fn calls(&self) -> impl ClientCallsApi + '_ {
        ClientCallsNamespace {
            motherboard: self.motherboard,
        }
    }

    fn stores(&self) -> impl ClientStoresApi + '_ {
        ClientStoresNamespace {
            motherboard: self.motherboard,
        }
    }

    fn fetch(&self) -> Result<InboxMessage, ClientError> {
        fetch(self.motherboard)
    }

    #[cfg(feature = "tokio")]
    fn fetch_async(
        &self,
    ) -> impl core::future::Future<Output = Result<InboxMessage, ClientError>> + '_ {
        self.motherboard.fetch_async()
    }
}

impl ServerApi for ServerNamespace<'_> {
    fn calls(&self) -> impl ServerCallsApi + '_ {
        ServerCallsNamespace {
            motherboard: self.motherboard,
        }
    }

    fn stores(&self) -> impl ServerStoresApi + '_ {
        ServerStoresNamespace {
            motherboard: self.motherboard,
        }
    }

    fn bind_service(&self, name: &str) -> Result<(), ClientError> {
        match self.execute(Command::BindService { name: name.into() })? {
            CommandReply::ServiceBound => Ok(()),
            reply => Err(ClientError::UnexpectedReply(reply)),
        }
    }

    fn register_service(&self, name: &str) -> Result<(), ClientError> {
        self.bind_service(name)
    }

    fn fetch(&self) -> Result<InboxMessage, ClientError> {
        fetch(self.motherboard)
    }

    #[cfg(feature = "tokio")]
    fn fetch_async(
        &self,
    ) -> impl core::future::Future<Output = Result<InboxMessage, ClientError>> + '_ {
        self.motherboard.fetch_async()
    }
}

impl ClientCallsApi for ClientCallsNamespace<'_> {
    fn call(
        &self,
        service: &str,
        method: &str,
        payload: impl Into<Box<[u8]>>,
        fds: impl Into<Box<[RawFd]>>,
    ) -> Result<RequestId, ClientError> {
        let request_id = self.motherboard.next_request_id();
        self.call_with_id(service, method, request_id, payload, fds)
    }

    fn call_with_id(
        &self,
        service: &str,
        method: &str,
        request_id: RequestId,
        payload: impl Into<Box<[u8]>>,
        fds: impl Into<Box<[RawFd]>>,
    ) -> Result<RequestId, ClientError> {
        match self.motherboard.execute(Command::FunctionCall {
            service: service.into(),
            method: method.into(),
            request_id,
            payload: payload.into(),
            fds: fds.into(),
        })? {
            CommandReply::FunctionCallAccepted {
                request_id: submitted,
            } if submitted == request_id => Ok(submitted),
            reply => Err(ClientError::UnexpectedReply(reply)),
        }
    }
}

impl ServerCallsApi for ServerCallsNamespace<'_> {
    fn reply(
        &self,
        reply_token: ReplyToken,
        status: ReplyStatus,
        payload: impl Into<Box<[u8]>>,
        fds: impl Into<Box<[RawFd]>>,
    ) -> Result<(), ClientError> {
        match self.motherboard.execute(Command::FunctionCallReply {
            reply_token,
            status,
            payload: payload.into(),
            fds: fds.into(),
        })? {
            CommandReply::FunctionCallReplyAccepted => Ok(()),
            reply => Err(ClientError::UnexpectedReply(reply)),
        }
    }
}

impl ClientStoresApi for ClientStoresNamespace<'_> {
    fn subscribe(
        &self,
        service: &str,
        store: &str,
        payload: impl Into<Box<[u8]>>,
    ) -> Result<SubscriptionId, ClientError> {
        let subscription_id = self.motherboard.next_subscription_id();
        self.subscribe_with_id(service, store, subscription_id, payload)
    }

    fn subscribe_with_id(
        &self,
        service: &str,
        store: &str,
        subscription_id: SubscriptionId,
        payload: impl Into<Box<[u8]>>,
    ) -> Result<SubscriptionId, ClientError> {
        match self.motherboard.execute(Command::StoreSubscribe {
            service: service.into(),
            store: store.into(),
            subscription_id,
            payload: payload.into(),
        })? {
            CommandReply::StoreSubscriptionAccepted {
                subscription_id: submitted,
            } if submitted == subscription_id => Ok(submitted),
            reply => Err(ClientError::UnexpectedReply(reply)),
        }
    }

    fn unsubscribe(&self, subscription_id: SubscriptionId) -> Result<(), ClientError> {
        match self
            .motherboard
            .execute(Command::StoreUnsubscribe { subscription_id })?
        {
            CommandReply::StoreUnsubscribed => Ok(()),
            reply => Err(ClientError::UnexpectedReply(reply)),
        }
    }
}

impl ServerStoresApi for ServerStoresNamespace<'_> {
    fn create(
        &self,
        service: &str,
        store: &str,
        initial_value: impl Into<Box<[u8]>>,
        public: bool,
    ) -> Result<(), ClientError> {
        match self.motherboard.execute(Command::StoreCreate {
            service: service.into(),
            store: store.into(),
            initial_value: initial_value.into(),
            public,
        })? {
            CommandReply::StoreCreateAccepted => Ok(()),
            reply => Err(ClientError::UnexpectedReply(reply)),
        }
    }

    fn update(
        &self,
        service: &str,
        store: &str,
        value: impl Into<Box<[u8]>>,
    ) -> Result<(), ClientError> {
        match self.motherboard.execute(Command::StoreUpdate {
            service: service.into(),
            store: store.into(),
            value: value.into(),
        })? {
            CommandReply::StoreUpdateAccepted => Ok(()),
            reply => Err(ClientError::UnexpectedReply(reply)),
        }
    }

    fn subscription_reply(
        &self,
        reply_token: ReplyToken,
        verdict: StoreSubscriptionServerVerdict,
    ) -> Result<(), ClientError> {
        match self.motherboard.execute(Command::StoreSubscriptionReply {
            reply_token,
            verdict,
        })? {
            CommandReply::StoreSubscriptionReplyAccepted => Ok(()),
            reply => Err(ClientError::UnexpectedReply(reply)),
        }
    }

    fn accept_subscription(&self, reply_token: ReplyToken) -> Result<(), ClientError> {
        self.subscription_reply(reply_token, StoreSubscriptionServerVerdict::Accepted)
    }

    fn reject_subscription(
        &self,
        reply_token: ReplyToken,
        message: impl Into<Box<str>>,
    ) -> Result<(), ClientError> {
        self.subscription_reply(
            reply_token,
            StoreSubscriptionServerVerdict::Rejected {
                message: message.into(),
            },
        )
    }
}

fn fetch(motherboard: &MotherboardClient) -> Result<InboxMessage, ClientError> {
    match motherboard.execute(Command::InboxNextMessage)? {
        CommandReply::InboxMessagePopped(message) => Ok(message),
        reply => Err(ClientError::UnexpectedReply(reply)),
    }
}

impl ServerNamespace<'_> {
    fn execute(&self, command: Command) -> Result<CommandReply, ClientError> {
        self.motherboard.execute(command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_ids_start_at_one() {
        let client = MotherboardClient {
            device: File::open("/dev/null").unwrap(),
            next_request_id: AtomicU64::new(1),
            next_subscription_id: AtomicU64::new(1),
        };

        assert_eq!(client.next_request_id(), RequestId(1));
        assert_eq!(client.next_request_id(), RequestId(2));
    }

    #[test]
    fn subscription_ids_start_at_one() {
        let client = MotherboardClient {
            device: File::open("/dev/null").unwrap(),
            next_request_id: AtomicU64::new(1),
            next_subscription_id: AtomicU64::new(1),
        };

        assert_eq!(client.next_subscription_id(), SubscriptionId(1));
        assert_eq!(client.next_subscription_id(), SubscriptionId(2));
    }
}
