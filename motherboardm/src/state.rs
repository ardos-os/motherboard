use hashbrown::HashMap;
use kernel::task::CurrentTask;
use lazy_static::lazy_static;
use motherboardm_protocol::{CommandResult, Origin, commands::*};
use spin::Mutex;

use crate::{
    SharedData, SharedStr,
    fake_files::inbox_latch::InboxLatch,
    motherboard_device::{FileId, MotherboardDevice},
    services::{
        QueuedRequest, RequestQueueError, Service, clone_files_from_raw_fds,
        store::{
            PendingStoreSubscription, StoreMap, StoreName, StorePath, StoreSubscriptionReplyTokens,
            StoreSubscriptionTarget, Subscriptions,
        },
    },
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

const DEBUG_DISABLE_BIND_SERVICE_UID_CHECK: bool = true;

pub struct State {
    services: HashMap<SharedStr, Service>,
    inboxes: Inboxes,
    reply_tokens: ReplyTokens,
    stores: StoreMap,
    subscriptions: Subscriptions,
    store_subscription_reply_tokens: StoreSubscriptionReplyTokens,
}
impl State {
    fn new() -> Self {
        Self {
            services: Default::default(),
            inboxes: Inboxes::new(),
            reply_tokens: ReplyTokens::new(),
            stores: StoreMap::new(),
            subscriptions: Subscriptions::new(),
            store_subscription_reply_tokens: StoreSubscriptionReplyTokens::new(),
        }
    }

    pub fn execute_command(
        &mut self,
        command: Command,
        auth_info: AuthInfo,
        file: FileId,
        process_cmdline: &str,
    ) -> CommandResult {
        match &command {
            Command::BindService { .. } => {
                self.execute_bind_service(command, auth_info, file, process_cmdline)
            }
            Command::FunctionCall { .. } => self.execute_function_call(command, auth_info, file),
            Command::FunctionCallReply { .. } => {
                self.execute_function_call_reply(command, auth_info, file)
            }
            Command::InboxNextMessage => self.execute_inbox_next_message(command, auth_info, file),
            Command::StoreSubscribe { .. } => {
                self.execute_store_subscribe(command, auth_info, file)
            }
            Command::AnonymousStoreSubscribe { .. } => {
                self.execute_anonymous_store_subscribe(command, auth_info, file)
            }
            Command::StoreSubscriptionReply { .. } => {
                self.execute_store_subscription_reply(command, auth_info, file)
            }
            Command::StoreCreate { .. } => self.execute_store_create(command, auth_info, file),
            Command::StoreUpdate { .. } => self.execute_store_update(command, auth_info, file),
            Command::AnonymousStoreCreate { .. } => {
                self.execute_anonymous_store_create(command, auth_info, file)
            }
            Command::AnonymousStoreUpdate { .. } => {
                self.execute_anonymous_store_update(command, auth_info, file)
            }
            Command::StoreUnsubscribe { .. } => {
                self.execute_store_unsubscribe(command, auth_info, file)
            }
        }
    }

    #[inline(never)]
    fn execute_bind_service(
        &mut self,
        command: Command,
        auth_info: AuthInfo,
        file: FileId,
        process_cmdline: &str,
    ) -> CommandResult {
        let Command::BindService { name } = command else {
            return CommandResult(Err(TransportError::ResourceExhausted));
        };

        // I think 1000 UIDs reserved for Ardos OS services is enough
        let is_system_daemon =
            auth_info.is_trusted && (DEBUG_DISABLE_BIND_SERVICE_UID_CHECK || auth_info.uid < 1000);
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
                message: "Service name is too long. Expected length between 0 and 30 characters"
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

    #[inline(never)]
    fn execute_function_call(
        &mut self,
        command: Command,
        auth_info: AuthInfo,
        file: FileId,
    ) -> CommandResult {
        let Command::FunctionCall {
            service,
            method,
            request_id,
            payload,
            fds,
        } = command
        else {
            return CommandResult(Err(TransportError::ResourceExhausted));
        };

        let Some(service) = self.services.get_mut(service.as_str()) else {
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
        CommandResult(Ok(CommandReply::FunctionCallAccepted { request_id }))
    }

    #[inline(never)]
    fn execute_function_call_reply(
        &mut self,
        command: Command,
        _auth_info: AuthInfo,
        file: FileId,
    ) -> CommandResult {
        let Command::FunctionCallReply {
            reply_token,
            status,
            payload,
            fds,
        } = command
        else {
            return CommandResult(Err(TransportError::ResourceExhausted));
        };

        match self.reply_tokens.consume(reply_token, file) {
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
                            InboxMessage::FunctionCallReply {
                                request_id: pending.request_id,
                                status,
                                payload,
                                fds: Array::default(),
                            },
                            stored_fds,
                        ),
                    )
                    .is_err()
                {
                    return CommandResult(Err(TransportError::ResourceExhausted));
                }
                CommandResult(Ok(CommandReply::FunctionCallReplyAccepted))
            }
            Err(error) => CommandResult(Err(error)),
        }
    }

    #[inline(never)]
    fn execute_inbox_next_message(
        &mut self,
        command: Command,
        _auth_info: AuthInfo,
        file: FileId,
    ) -> CommandResult {
        let Command::InboxNextMessage = command else {
            return CommandResult(Err(TransportError::ResourceExhausted));
        };

        match self.inboxes.fetch(file) {
            Ok(Some(message)) => {
                return CommandResult(Ok(CommandReply::InboxMessagePopped(message)));
            }
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
            Ok(first) => CommandResult(Ok(CommandReply::InboxMessagePopped(first))),
            Err(_) => CommandResult(Err(TransportError::ResourceExhausted)),
        }
    }

    #[inline(never)]
    fn execute_store_subscribe(
        &mut self,
        command: Command,
        auth_info: AuthInfo,
        file: FileId,
    ) -> CommandResult {
        let Command::StoreSubscribe {
            service,
            store,
            subscription_id,
        } = command
        else {
            return CommandResult(Err(TransportError::ResourceExhausted));
        };

        let Some(service_ref) = self.services.get(service.as_str()) else {
            return CommandResult(Err(TransportError::NoSuchService));
        };
        let store_name = match StoreName::check(store) {
            Ok(store_name) => store_name,
            Err(error) => {
                return CommandResult(Err(TransportError::InvalidStoreName {
                    message: error.message.as_str().into(),
                }));
            }
        };
        let path = StorePath::new(service, store_name);
        let Some(store_ref) = self.stores.get(&path) else {
            return CommandResult(Err(TransportError::NoSuchStore));
        };

        if store_ref.is_public() {
            if let Err(error) = self.subscriptions.insert(
                subscription_id,
                file,
                StoreSubscriptionTarget::Named(path.clone()),
            ) {
                return CommandResult(Err(error));
            }

            let snapshot = store_ref.snapshot();
            if self
                .inboxes
                .queue(
                    file,
                    QueuedInboxMessage::without_fds(InboxMessage::StoreSubscriptionAccepted {
                        service: snapshot.path.service.as_str().into(),
                        store: snapshot.path.store.as_str().into(),
                        subscription_id,
                        current_value: snapshot.current_value.as_ref().into(),
                        last_updated_timestamp: snapshot.last_updated_timestamp,
                    }),
                )
                .is_err()
            {
                return CommandResult(Err(TransportError::ResourceExhausted));
            }
        } else {
            let reply_token = self.store_subscription_reply_tokens.create(
                file,
                service_ref.file_id,
                subscription_id,
                StoreSubscriptionTarget::Named(path.clone()),
            );
            if self
                .inboxes
                .queue(
                    service_ref.file_id,
                    QueuedInboxMessage::without_fds(InboxMessage::SubscribeRequest {
                        service: path.service.as_str().into(),
                        store: path.store.as_str().into(),
                        subscription_id,
                        reply_token,
                        origin: auth_info.into(),
                    }),
                )
                .is_err()
            {
                return CommandResult(Err(TransportError::ResourceExhausted));
            }
        }

        CommandResult(Ok(CommandReply::StoreSubscriptionAccepted {
            subscription_id,
        }))
    }

    #[inline(never)]
    fn execute_anonymous_store_subscribe(
        &mut self,
        command: Command,
        auth_info: AuthInfo,
        file: FileId,
    ) -> CommandResult {
        let Command::AnonymousStoreSubscribe {
            id,
            subscription_id,
        } = command
        else {
            return CommandResult(Err(TransportError::ResourceExhausted));
        };

        let Some(store_ref) = self.stores.get_anonymous(id) else {
            return CommandResult(Err(TransportError::NoSuchStore));
        };
        let service_file_id = store_ref.owner_file_id();

        if self.stores.increment_anonymous_pending(id).is_none() {
            return CommandResult(Err(TransportError::NoSuchStore));
        }

        let reply_token = self.store_subscription_reply_tokens.create(
            file,
            service_file_id,
            subscription_id,
            StoreSubscriptionTarget::Anonymous(id),
        );
        if self
            .inboxes
            .queue(
                service_file_id,
                QueuedInboxMessage::without_fds(InboxMessage::AnonymousStoreSubscribeRequest {
                    id,
                    subscription_id,
                    reply_token,
                    origin: auth_info.into(),
                }),
            )
            .is_err()
        {
            self.store_subscription_reply_tokens
                .consume(reply_token, service_file_id)
                .ok();
            self.stores.decrement_anonymous_pending(id);
            return CommandResult(Err(TransportError::ResourceExhausted));
        }

        CommandResult(Ok(CommandReply::StoreSubscriptionAccepted {
            subscription_id,
        }))
    }

    #[inline(never)]
    fn execute_store_subscription_reply(
        &mut self,
        command: Command,
        _auth_info: AuthInfo,
        file: FileId,
    ) -> CommandResult {
        let Command::StoreSubscriptionReply {
            reply_token,
            verdict,
        } = command
        else {
            return CommandResult(Err(TransportError::ResourceExhausted));
        };

        let pending = match self
            .store_subscription_reply_tokens
            .consume(reply_token, file)
        {
            Ok(pending) => pending,
            Err(error) => return CommandResult(Err(error)),
        };

        match verdict {
            StoreSubscriptionServerVerdict::Accepted => {
                let snapshot = match &pending.target {
                    StoreSubscriptionTarget::Named(path) => {
                        let Some(store_ref) = self.stores.get(path) else {
                            return CommandResult(Err(TransportError::NoSuchStore));
                        };
                        store_ref.snapshot()
                    }
                    StoreSubscriptionTarget::Anonymous(id) => {
                        if self.stores.accept_anonymous_pending(*id).is_none() {
                            return CommandResult(Err(TransportError::NoSuchStore));
                        }
                        let Some(store_ref) = self.stores.get_anonymous(*id) else {
                            return CommandResult(Err(TransportError::NoSuchStore));
                        };
                        let snapshot = store_ref.snapshot();
                        if let Err(error) = self.subscriptions.insert(
                            pending.subscription_id,
                            pending.client_file_id,
                            pending.target.clone(),
                        ) {
                            self.stores.decrement_anonymous_accepted(*id);
                            return CommandResult(Err(error));
                        }

                        if self
                            .inboxes
                            .queue(
                                pending.client_file_id,
                                QueuedInboxMessage::without_fds(
                                    InboxMessage::AnonymousStoreSubscriptionAccepted {
                                        id: snapshot.id,
                                        subscription_id: pending.subscription_id,
                                        current_value: snapshot.current_value.as_ref().into(),
                                        last_updated_timestamp: snapshot.last_updated_timestamp,
                                    },
                                ),
                            )
                            .is_err()
                        {
                            let _ = self
                                .subscriptions
                                .delete_owned(pending.subscription_id, pending.client_file_id);
                            self.stores.decrement_anonymous_accepted(*id);
                            return CommandResult(Err(TransportError::ResourceExhausted));
                        }

                        return CommandResult(Ok(CommandReply::StoreSubscriptionReplyAccepted));
                    }
                };
                if let Err(error) = self.subscriptions.insert(
                    pending.subscription_id,
                    pending.client_file_id,
                    pending.target.clone(),
                ) {
                    return CommandResult(Err(error));
                }

                if self
                    .inboxes
                    .queue(
                        pending.client_file_id,
                        QueuedInboxMessage::without_fds(InboxMessage::StoreSubscriptionAccepted {
                            service: snapshot.path.service.as_str().into(),
                            store: snapshot.path.store.as_str().into(),
                            subscription_id: pending.subscription_id,
                            current_value: snapshot.current_value.as_ref().into(),
                            last_updated_timestamp: snapshot.last_updated_timestamp,
                        }),
                    )
                    .is_err()
                {
                    return CommandResult(Err(TransportError::ResourceExhausted));
                }
            }
            StoreSubscriptionServerVerdict::Rejected { message } => match &pending.target {
                StoreSubscriptionTarget::Named(path) => {
                    if self
                        .inboxes
                        .queue(
                            pending.client_file_id,
                            QueuedInboxMessage::without_fds(
                                InboxMessage::StoreSubscriptionRejected {
                                    service: path.service.as_str().into(),
                                    store: path.store.as_str().into(),
                                    subscription_id: pending.subscription_id,
                                    message,
                                },
                            ),
                        )
                        .is_err()
                    {
                        return CommandResult(Err(TransportError::ResourceExhausted));
                    }
                }
                StoreSubscriptionTarget::Anonymous(id) => {
                    self.stores.decrement_anonymous_pending(*id);
                    if self
                        .inboxes
                        .queue(
                            pending.client_file_id,
                            QueuedInboxMessage::without_fds(
                                InboxMessage::AnonymousStoreSubscriptionRejected {
                                    id: *id,
                                    subscription_id: pending.subscription_id,
                                    message,
                                },
                            ),
                        )
                        .is_err()
                    {
                        return CommandResult(Err(TransportError::ResourceExhausted));
                    }
                }
            },
        }

        CommandResult(Ok(CommandReply::StoreSubscriptionReplyAccepted))
    }

    #[inline(never)]
    fn execute_store_create(
        &mut self,
        command: Command,
        _auth_info: AuthInfo,
        file: FileId,
    ) -> CommandResult {
        let Command::StoreCreate {
            service,
            store,
            initial_value,
            public,
        } = command
        else {
            return CommandResult(Err(TransportError::ResourceExhausted));
        };

        let Some(service_ref) = self.services.get(service.as_str()) else {
            return CommandResult(Err(TransportError::NoSuchService));
        };
        if service_ref.file_id != file {
            return CommandResult(Err(TransportError::Unauthorized));
        }

        let store_name = match StoreName::check(store) {
            Ok(store_name) => store_name,
            Err(error) => {
                return CommandResult(Err(TransportError::InvalidStoreName {
                    message: error.message.as_str().into(),
                }));
            }
        };
        let path = StorePath::new(service, store_name);
        if self
            .stores
            .try_register(path, initial_value.into(), public)
            .is_err()
        {
            return CommandResult(Err(TransportError::StoreAlreadyExists));
        }

        CommandResult(Ok(CommandReply::StoreCreateAccepted))
    }

    #[inline(never)]
    fn execute_store_update(
        &mut self,
        command: Command,
        _auth_info: AuthInfo,
        file: FileId,
    ) -> CommandResult {
        let Command::StoreUpdate {
            service,
            store,
            value,
        } = command
        else {
            return CommandResult(Err(TransportError::ResourceExhausted));
        };

        let Some(service_ref) = self.services.get(service.as_str()) else {
            return CommandResult(Err(TransportError::NoSuchService));
        };
        if service_ref.file_id != file {
            return CommandResult(Err(TransportError::Unauthorized));
        }

        let store_name = match StoreName::check(store) {
            Ok(store_name) => store_name,
            Err(error) => {
                return CommandResult(Err(TransportError::InvalidStoreName {
                    message: error.message.as_str().into(),
                }));
            }
        };
        let path = StorePath::new(service, store_name);
        let Some(snapshot) = self.stores.update(&path, value.into()) else {
            return CommandResult(Err(TransportError::NoSuchStore));
        };

        for (subscriber_file, subscription_id) in self.subscriptions.subscribers_for_store(&path) {
            if self
                .inboxes
                .queue(
                    subscriber_file,
                    QueuedInboxMessage::without_fds(InboxMessage::StoreSubscriptionUpdated {
                        service: snapshot.path.service.as_str().into(),
                        store: snapshot.path.store.as_str().into(),
                        subscription_id,
                        payload: snapshot.current_value.as_ref().into(),
                    }),
                )
                .is_err()
            {
                return CommandResult(Err(TransportError::ResourceExhausted));
            }
        }

        CommandResult(Ok(CommandReply::StoreUpdateAccepted))
    }

    #[inline(never)]
    fn execute_anonymous_store_create(
        &mut self,
        command: Command,
        _auth_info: AuthInfo,
        file: FileId,
    ) -> CommandResult {
        let Command::AnonymousStoreCreate {
            service,
            initial_value,
        } = command
        else {
            return CommandResult(Err(TransportError::ResourceExhausted));
        };

        let Some(service_ref) = self.services.get(service.as_str()) else {
            return CommandResult(Err(TransportError::NoSuchService));
        };
        if service_ref.file_id != file {
            return CommandResult(Err(TransportError::Unauthorized));
        }

        let snapshot = self
            .stores
            .create_anonymous(service.into(), file, initial_value.into());
        CommandResult(Ok(CommandReply::AnonymousStoreCreated { id: snapshot.id }))
    }

    #[inline(never)]
    fn execute_anonymous_store_update(
        &mut self,
        command: Command,
        _auth_info: AuthInfo,
        file: FileId,
    ) -> CommandResult {
        let Command::AnonymousStoreUpdate { id, value } = command else {
            return CommandResult(Err(TransportError::ResourceExhausted));
        };

        let Some(store_ref) = self.stores.get_anonymous(id) else {
            return CommandResult(Err(TransportError::NoSuchStore));
        };
        if store_ref.owner_file_id() != file {
            return CommandResult(Err(TransportError::Unauthorized));
        }

        let Some(snapshot) = self.stores.update_anonymous(id, value.into()) else {
            return CommandResult(Err(TransportError::NoSuchStore));
        };

        for (subscriber_file, subscription_id) in
            self.subscriptions.subscribers_for_anonymous_store(id)
        {
            if self
                .inboxes
                .queue(
                    subscriber_file,
                    QueuedInboxMessage::without_fds(
                        InboxMessage::AnonymousStoreSubscriptionUpdated {
                            id: snapshot.id,
                            subscription_id,
                            payload: snapshot.current_value.as_ref().into(),
                        },
                    ),
                )
                .is_err()
            {
                return CommandResult(Err(TransportError::ResourceExhausted));
            }
        }

        CommandResult(Ok(CommandReply::StoreUpdateAccepted))
    }

    #[inline(never)]
    fn execute_store_unsubscribe(
        &mut self,
        command: Command,
        _auth_info: AuthInfo,
        file: FileId,
    ) -> CommandResult {
        let Command::StoreUnsubscribe { subscription_id } = command else {
            return CommandResult(Err(TransportError::ResourceExhausted));
        };

        let target = match self.subscriptions.delete_owned(subscription_id, file) {
            Ok(target) => target,
            Err(_) => return CommandResult(Err(TransportError::NoSuchSubscription)),
        };

        if let StoreSubscriptionTarget::Anonymous(id) = target {
            if let Some(store_ref) = self.stores.get_anonymous(id) {
                let _ = self.inboxes.queue(
                    store_ref.owner_file_id(),
                    QueuedInboxMessage::without_fds(
                        InboxMessage::AnonymousStoreSubscriptionClosed {
                            id,
                            subscription_id,
                            reason: CloseReason::Closed,
                        },
                    ),
                );
            }
            self.stores.decrement_anonymous_accepted(id);
        }

        CommandResult(Ok(CommandReply::StoreUnsubscribed))
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
        for pending in self
            .store_subscription_reply_tokens
            .remove_for_file(device.file_id())
        {
            self.close_pending_subscription(pending, device.file_id(), CloseReason::Closed);
        }
        for (subscriber_file, subscription_id, target) in
            self.subscriptions.cleanup_file(device.file_id())
        {
            self.close_active_subscription(
                subscriber_file,
                subscription_id,
                target,
                device.file_id(),
                CloseReason::Closed,
            );
        }

        if !closed_services.is_empty() {
            log::info!(
                "service provider at file id {} (auth_info={:#?}) terminated bringing along the following services with it: {:#?} ",
                device.file_id(),
                device.auth_info,
                closed_services
            );
            for service in closed_services {
                let service: &str = service.as_str();
                self.stores.remove_service(service);
                for (subscriber_file, subscription_id, target) in
                    self.subscriptions.cleanup_service(service)
                {
                    self.close_active_subscription(
                        subscriber_file,
                        subscription_id,
                        target,
                        device.file_id(),
                        CloseReason::ServiceExited,
                    );
                }
                for id in self.stores.remove_anonymous_service(service) {
                    for (subscriber_file, subscription_id, target) in
                        self.subscriptions.cleanup_anonymous_store(id)
                    {
                        self.close_active_subscription(
                            subscriber_file,
                            subscription_id,
                            target,
                            device.file_id(),
                            CloseReason::ServiceExited,
                        );
                    }
                }
                self.inboxes.broadcast(QueuedInboxMessage::without_fds(
                    InboxMessage::ServiceClosed {
                        service: service.into(),
                        reason: CloseReason::ServiceExited,
                    },
                ));
            }
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

    fn close_pending_subscription(
        &mut self,
        pending: PendingStoreSubscription,
        closed_file: FileId,
        reason: CloseReason,
    ) {
        if let StoreSubscriptionTarget::Anonymous(id) = pending.target {
            self.stores.decrement_anonymous_pending(id);
            if pending.client_file_id != closed_file {
                let _ = self.inboxes.queue(
                    pending.client_file_id,
                    QueuedInboxMessage::without_fds(
                        InboxMessage::AnonymousStoreSubscriptionClosed {
                            id,
                            subscription_id: pending.subscription_id,
                            reason: reason.clone(),
                        },
                    ),
                );
            }
            if pending.service_file_id != closed_file {
                let _ = self.inboxes.queue(
                    pending.service_file_id,
                    QueuedInboxMessage::without_fds(
                        InboxMessage::AnonymousStoreSubscriptionClosed {
                            id,
                            subscription_id: pending.subscription_id,
                            reason,
                        },
                    ),
                );
            }
        }
    }

    fn close_active_subscription(
        &mut self,
        subscriber_file: FileId,
        subscription_id: SubscriptionId,
        target: StoreSubscriptionTarget,
        closed_file: FileId,
        reason: CloseReason,
    ) {
        match target {
            StoreSubscriptionTarget::Named(path) => {
                if subscriber_file != closed_file {
                    let _ = self.inboxes.queue(
                        subscriber_file,
                        QueuedInboxMessage::without_fds(InboxMessage::StoreSubscriptionClosed {
                            service: path.service.as_str().into(),
                            store: path.store.as_str().into(),
                            subscription_id,
                            reason,
                        }),
                    );
                }
            }
            StoreSubscriptionTarget::Anonymous(id) => {
                let owner_file_id = self
                    .stores
                    .get_anonymous(id)
                    .map(|store| store.owner_file_id());
                if subscriber_file != closed_file {
                    let _ = self.inboxes.queue(
                        subscriber_file,
                        QueuedInboxMessage::without_fds(
                            InboxMessage::AnonymousStoreSubscriptionClosed {
                                id,
                                subscription_id,
                                reason: reason.clone(),
                            },
                        ),
                    );
                }
                if let Some(owner_file_id) = owner_file_id {
                    if owner_file_id != closed_file {
                        let _ = self.inboxes.queue(
                            owner_file_id,
                            QueuedInboxMessage::without_fds(
                                InboxMessage::AnonymousStoreSubscriptionClosed {
                                    id,
                                    subscription_id,
                                    reason,
                                },
                            ),
                        );
                    }
                    self.stores.decrement_anonymous_accepted(id);
                }
            }
        }
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
