use alloc::vec::Vec;
use hashbrown::HashMap;
use kernel::task::CurrentTask;
use lazy_static::lazy_static;
use motherboardm_protocol::{CommandResult, Origin, commands::*};
use spin::Mutex;

use crate::{
    SharedData, SharedStr,
    fake_files::inbox_latch::InboxLatch,
    motherboard_device::{FileId, MotherboardDevice},
    services::{QueuedRequest, RequestQueueError, Service, clone_files_from_raw_fds},
    state::{
        message_inbox::{Inboxes, QueuedInboxMessage},
        reply_tokens::ReplyTokens,
    },
};
#[derive(Hash, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PidWithNs {
    pub namespace: usize,
    pub pid: i32,
}
impl<'a> From<&'a CurrentTask> for PidWithNs {
    fn from(value: &'a CurrentTask) -> Self {
        Self {
            namespace: value
                .get_pid_ns()
                .map_or_else(core::ptr::null_mut, |p| p.as_ptr())
                .addr(),
            pid: value.pid(),
        }
    }
}
#[derive(Debug, Clone, Copy)]
pub struct AuthInfo {
    pub uid: u32,
    pub pid_with_ns: PidWithNs,
    pub is_trusted: bool,
}

pub mod message_inbox;
pub mod reply_tokens;

pub struct State {
    services: HashMap<SharedStr, Service>,
    inboxes: Inboxes,
    reply_tokens: ReplyTokens,
}
impl State {
    fn new() -> Self {
        Self {
            services: Default::default(),
            inboxes: Inboxes::new(),
            reply_tokens: ReplyTokens::new(),
        }
    }

    pub fn execute_command(
        &mut self,
        command: Command,
        auth_info: AuthInfo,
        file: FileId,
        process_cmdline: &str,
    ) -> CommandResult {
        let todo = CommandResult(Err(TransportError::NotImplemented));
        match command {
            Command::BindService { name } => {
                // I think 1000 UIDs reserved for Ardos OS services is enough
                let is_system_daemon = auth_info.is_trusted && auth_info.uid < 1000;
                if !is_system_daemon {
                    return CommandResult(Err(TransportError::Unauthorized));
                }
                if name.contains(" ") {
                    return CommandResult(Err(TransportError::InvalidServiceName {
                        message: "Service names cannot contain spaces".into(),
                    }));
                }
                if name.len() > 30 {
                    return CommandResult(Err(TransportError::InvalidServiceName {
                        message:
                            "Service name is too long. Expected length between 0 and 30 characters"
                                .into(),
                    }));
                }

                if !name.chars().all(|c| c.is_ascii_alphanumeric()) {
                    return CommandResult(Err(TransportError::InvalidServiceName {
                        message: "Service names must only contain letters and numbers".into(),
                    }));
                }
                let name: SharedStr = name.into();
                if self.services.contains_key(&name) {
                    return CommandResult(Err(TransportError::ServiceNameConflict));
                }

                self.services
                    .insert(name.clone(), Service::new(name.clone(), auth_info, file));

                log::debug!(
                    "Process with PID {} (command={:?}) as user {} is providing service {} through file {}",
                    auth_info.pid_with_ns.pid,
                    process_cmdline,
                    auth_info.uid,
                    name,
                    file
                );
                CommandResult(Ok(CommandReply::ServiceBound))
            }
            Command::Call {
                service,
                method,
                request_id,
                payload,
                fds,
            } => {
                let Some(service) = self.services.get_mut(service.as_ref()) else {
                    return CommandResult(Err(TransportError::NoSuchService));
                };
                let payload: SharedData = payload.into();
                let origin = auth_info.into();
                let queued_request =
                    match QueuedRequest::new(file, origin, method, request_id, payload, fds) {
                        Ok(queued_request) => queued_request,
                        Err(RequestQueueError::InvalidFds(_)) => {
                            return CommandResult(Err(TransportError::InvalidFileDescriptor));
                        }
                    };

                let service_file_id = service.file_id;
                service.queue_call(queued_request);
                self.inboxes.notify(service_file_id);
                CommandResult(Ok(CommandReply::Submitted { request_id }))
            }
            Command::Reply {
                reply_token,
                status,
                payload,
                fds,
            } => match self.reply_tokens.consume(reply_token, file) {
                Ok(pending) => {
                    let stored_fds = match clone_files_from_raw_fds(fds) {
                        Ok(stored_fds) => stored_fds,
                        Err(RequestQueueError::InvalidFds(_)) => {
                            return CommandResult(Err(TransportError::InvalidFileDescriptor));
                        }
                    };

                    if self
                        .inboxes
                        .queue(
                            pending.client_file_id,
                            QueuedInboxMessage::new(
                                InboxMessage::CallReply {
                                    request_id: pending.request_id,
                                    status,
                                    payload,
                                    fds: Vec::new().into_boxed_slice(),
                                },
                                stored_fds,
                            ),
                        )
                        .is_err()
                    {
                        return CommandResult(Err(TransportError::ResourceExhausted));
                    }
                    CommandResult(Ok(CommandReply::Replied))
                }
                Err(error) => CommandResult(Err(error)),
            },
            Command::Subscribe { .. } => todo,
            Command::SubscriptionReply { .. } => todo,
            Command::UpdateStore { .. } => todo,
            Command::Cancel { .. } => todo,
            Command::Fetch => {
                match self.inboxes.fetch(file) {
                    Ok(Some(message)) => return CommandResult(Ok(CommandReply::Message(message))),
                    Ok(None) => {}
                    Err(_) => return CommandResult(Err(TransportError::ResourceExhausted)),
                }

                let Some(service) = self
                    .services
                    .values_mut()
                    .find(|service| service.file_id == file)
                else {
                    return self.would_block(file);
                };

                let messages = service.flush_requests_as_inbox_messages(&mut self.reply_tokens);
                let mut messages = messages.into_vec();
                if messages.is_empty() {
                    return self.would_block(file);
                }

                let first = messages.remove(0);
                for message in messages {
                    if self.inboxes.queue(file, message).is_err() {
                        return CommandResult(Err(TransportError::ResourceExhausted));
                    }
                }

                match first.into_user_message() {
                    Ok(first) => CommandResult(Ok(CommandReply::Message(first))),
                    Err(_) => CommandResult(Err(TransportError::ResourceExhausted)),
                }
            }
        }
    }
    pub fn cleanup(&mut self, device: &MotherboardDevice) {
        self.inboxes.close_inbox(device.file_id());
        let mut closed_services = alloc::vec![];
        self.services.retain(|_, s| {
            let matches_file = s.file_id == device.file_id();
            if matches_file {
                closed_services.push(s.name.clone());
            }
            !matches_file
        });
        self.reply_tokens.remove_for_file(device.file_id());

        if !closed_services.is_empty() {
            log::info!(
                "service provider at file id {} (auth_info={:#?}) terminated bringing along the following services with it: {:#?} ",
                device.file_id(),
                device.auth_info,
                closed_services
            );
        } else {
            log::info!(
                "client at file id {} (auth_info={:#?}) terminated",
                device.file_id(),
                device.auth_info
            );
        }
    }
    pub fn is_initialized() -> bool {
        GLOBAL_STATE.lock().is_some()
    }
    pub fn inboxes(&self) -> &Inboxes {
        &self.inboxes
    }
    fn would_block(&mut self, file: FileId) -> CommandResult {
        let Ok(generation) = self.inboxes.latch_generation(file) else {
            return CommandResult(Err(TransportError::ResourceExhausted));
        };
        let Ok(latch_fd) = InboxLatch::new(file, generation) else {
            return CommandResult(Err(TransportError::ResourceExhausted));
        };

        CommandResult(Err(TransportError::WouldBlock { latch_fd }))
    }
    pub fn use_state<T: Sized>(cb: impl FnOnce(&mut State) -> T) -> T {
        let mut lock = GLOBAL_STATE.lock();
        let opt = lock.as_mut().expect("state to be initialized");
        (cb)(opt)
    }
    pub fn drop() {
        if Self::is_initialized() {
            *GLOBAL_STATE.lock() = None;
        }
    }
}
impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}
lazy_static! {
    static ref GLOBAL_STATE: Mutex<Option<State>> = Mutex::new(Some(State::new()));
}

impl From<AuthInfo> for Origin {
    fn from(value: AuthInfo) -> Self {
        Self {
            pid: value.pid_with_ns.pid as u32,
            uid: value.uid,
            gid: 0,
            is_trusted: value.is_trusted,
        }
    }
}
