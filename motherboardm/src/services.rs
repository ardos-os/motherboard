use alloc::{collections::vec_deque::VecDeque, vec::Vec};
use kernel::{
    fs::{File, LocalFile, file::BadFdError},
    sync::aref::ARef,
};
use motherboardm_protocol::{InboxMessage, Origin, RawFd, RequestId, commands::Array};

use crate::{
    SharedData, SharedStr,
    motherboard_device::FileId,
    state::{AuthInfo, message_inbox::QueuedInboxMessage, reply_tokens::ReplyTokens},
};
pub struct QueuedRequest {
    client_file_id: FileId,
    origin: Origin,
    method: SharedStr,
    request_id: RequestId,
    payload: SharedData,
    fds: Array<ARef<File>>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RequestQueueError {
    InvalidFds(BadFdError),
}

impl QueuedRequest {
    pub fn new(
        client_file_id: FileId,
        origin: Origin,
        method: impl Into<SharedStr>,
        request_id: RequestId,
        payload: SharedData,
        fds: impl AsRef<[RawFd]>,
    ) -> Result<Self, RequestQueueError> {
        let fds = clone_files_from_raw_fds(fds)?;
        Ok(Self {
            client_file_id,
            origin,
            fds: fds.into(),
            method: method.into(),
            payload: payload,
            request_id,
        })
    }
}

pub fn clone_files_from_raw_fds(
    fds: impl AsRef<[RawFd]>,
) -> Result<Array<ARef<File>>, RequestQueueError> {
    fds.as_ref()
        .iter()
        .map(|fd| LocalFile::fget(*fd).map(|f| unsafe { LocalFile::assume_no_fdget_pos(f) }))
        .collect::<Result<Vec<_>, BadFdError>>()
        .map(Into::into)
        .map_err(RequestQueueError::InvalidFds)
}
pub mod store;
pub struct Service {
    /// Points to the original open connection and thus the original process who registered this service
    pub file_id: FileId,
    /// Contains data about who registered the service (uid and pid)
    pub auth_info: AuthInfo,

    request_queue: VecDeque<QueuedRequest>,
    pub name: SharedStr,
}
impl Service {
    pub fn new(
        name: impl Into<SharedStr>,
        auth_info: AuthInfo,
        file_id: impl Into<FileId>,
    ) -> Self {
        Self {
            file_id: file_id.into(),
            auth_info,
            request_queue: VecDeque::new(),
            name: name.into(),
        }
    }
    pub fn queue_call(&mut self, call: QueuedRequest) {
        self.request_queue.push_back(call);
    }
    pub fn flush_requests_as_inbox_messages(
        &mut self,
        reply_tokens: &mut ReplyTokens,
    ) -> Array<QueuedInboxMessage> {
        let service_file_id = self.file_id;
        let service_name = self.name.clone();
        self.request_queue
            .drain(..)
            .map(|qr| {
                QueuedInboxMessage::new(
                    InboxMessage::FunctionCallRequest {
                        service: service_name.as_str().into(),
                        method: qr.method.as_str().into(),
                        request_id: qr.request_id,
                        reply_token: reply_tokens.create(
                            qr.client_file_id,
                            service_file_id,
                            qr.request_id,
                        ),
                        origin: qr.origin,
                        payload: qr.payload.as_ref().into(),
                        fds: Vec::new().into(),
                    },
                    qr.fds,
                )
            })
            .collect()
    }
}
