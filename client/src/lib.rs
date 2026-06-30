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
    ReplyToken, RequestId, SubscriptionId, TransportError,
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

    pub fn bind_service(&self, name: &str) -> Result<(), ClientError> {
        match self.execute(Command::BindService { name: name.into() })? {
            CommandReply::ServiceBound => Ok(()),
            reply => Err(ClientError::UnexpectedReply(reply)),
        }
    }

    pub fn register_service(&self, name: &str) -> Result<(), ClientError> {
        self.bind_service(name)
    }

    pub fn call(
        &self,
        service: &str,
        method: &str,
        payload: impl Into<Box<[u8]>>,
        fds: impl Into<Box<[RawFd]>>,
    ) -> Result<RequestId, ClientError> {
        let request_id = self.next_request_id();
        self.call_with_id(service, method, request_id, payload, fds)
    }

    pub fn call_with_id(
        &self,
        service: &str,
        method: &str,
        request_id: RequestId,
        payload: impl Into<Box<[u8]>>,
        fds: impl Into<Box<[RawFd]>>,
    ) -> Result<RequestId, ClientError> {
        match self.execute(Command::Call {
            service: service.into(),
            method: method.into(),
            request_id,
            payload: payload.into(),
            fds: fds.into(),
        })? {
            CommandReply::Submitted {
                request_id: submitted,
            } if submitted == request_id => Ok(submitted),
            reply => Err(ClientError::UnexpectedReply(reply)),
        }
    }

    pub fn reply(
        &self,
        reply_token: ReplyToken,
        status: ReplyStatus,
        payload: impl Into<Box<[u8]>>,
        fds: impl Into<Box<[RawFd]>>,
    ) -> Result<(), ClientError> {
        match self.execute(Command::Reply {
            reply_token,
            status,
            payload: payload.into(),
            fds: fds.into(),
        })? {
            CommandReply::Replied => Ok(()),
            reply => Err(ClientError::UnexpectedReply(reply)),
        }
    }

    pub fn subscribe(
        &self,
        service: &str,
        topic: &str,
        payload: impl Into<Box<[u8]>>,
    ) -> Result<SubscriptionId, ClientError> {
        let subscription_id = self.next_subscription_id();
        self.subscribe_with_id(service, topic, subscription_id, payload)
    }

    pub fn subscribe_with_id(
        &self,
        service: &str,
        store: &str,
        subscription_id: SubscriptionId,
        payload: impl Into<Box<[u8]>>,
    ) -> Result<SubscriptionId, ClientError> {
        match self.execute(Command::Subscribe {
            service: service.into(),
            store: store.into(),
            subscription_id,
            payload: payload.into(),
        })? {
            CommandReply::SubscriptionSubmitted {
                subscription_id: submitted,
            } if submitted == subscription_id => Ok(submitted),
            reply => Err(ClientError::UnexpectedReply(reply)),
        }
    }

    pub fn subscription_reply(
        &self,
        reply_token: ReplyToken,
        accepted: bool,
        payload: impl Into<Box<[u8]>>,
    ) -> Result<(), ClientError> {
        match self.execute(Command::SubscriptionReply {
            reply_token,
            accepted,
            payload: payload.into(),
        })? {
            CommandReply::SubscriptionReplied => Ok(()),
            reply => Err(ClientError::UnexpectedReply(reply)),
        }
    }

    pub fn update_store(
        &self,
        store: impl Into<Box<str>>,
        payload: impl Into<Box<[u8]>>,
        fds: impl Into<Box<[RawFd]>>,
    ) -> Result<(), ClientError> {
        match self.execute(Command::UpdateStore {
            store: store.into(),
            payload: payload.into(),
            fds: fds.into(),
        })? {
            CommandReply::Emitted => Ok(()),
            reply => Err(ClientError::UnexpectedReply(reply)),
        }
    }

    pub fn cancel(&self, subscription_id: SubscriptionId) -> Result<(), ClientError> {
        match self.execute(Command::Cancel { subscription_id })? {
            CommandReply::Cancelled => Ok(()),
            reply => Err(ClientError::UnexpectedReply(reply)),
        }
    }

    pub fn fetch(&self) -> Result<InboxMessage, ClientError> {
        match self.execute(Command::Fetch)? {
            CommandReply::Message(message) => Ok(message),
            reply => Err(ClientError::UnexpectedReply(reply)),
        }
    }

    #[cfg(feature = "tokio")]
    pub async fn fetch_async(&self) -> Result<InboxMessage, ClientError> {
        loop {
            match self.fetch() {
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
