use alloc::{collections::vec_deque::VecDeque, vec::Vec};
use core::pin::Pin;

use hashbrown::HashMap;
use kernel::{
    alloc::KBox,
    fs::{File, file::FileDescriptorReservation},
    prelude::*,
    sync::poll::PollCondVar,
    sync::aref::ARef,
};
use motherboardm_protocol::{InboxMessage, RawFd, commands::Array};

use crate::{
    fake_files::{FakeFilePoll, FakeFilePollEvents},
    motherboard_device::FileId,
};

pub struct Message(pub QueuedInboxMessage);
#[derive(Clone)]
pub struct QueuedInboxMessage {
    message: InboxMessage,
    fds: Array<ARef<File>>,
}

impl QueuedInboxMessage {
    pub fn new(message: InboxMessage, fds: Array<ARef<File>>) -> Self {
        Self { message, fds }
    }

    pub fn without_fds(message: InboxMessage) -> Self {
        Self {
            message,
            fds: Vec::new().into(),
        }
    }

    pub fn into_user_message(mut self) -> Result<InboxMessage> {
        let installed_fds = install_fds(self.fds)?;

        match &mut self.message {
            InboxMessage::FunctionCallRequest { fds, .. }
            | InboxMessage::FunctionCallReply { fds, .. } => {
                *fds = installed_fds;
            }
            _ => {}
        }

        Ok(self.message)
    }
}

fn install_fds(files: Array<ARef<File>>) -> Result<Array<RawFd>> {
    let mut reservations = Vec::new();
    for _ in files.iter() {
        reservations.push(FileDescriptorReservation::get_unused_fd_flags(0)?);
    }

    let mut raw_fds = Vec::new();
    for reservation in reservations.iter() {
        raw_fds.push(reservation.reserved_fd());
    }

    for (reservation, file) in reservations.into_iter().zip(files.into_vec()) {
        reservation.fd_install(file);
    }

    Ok(raw_fds.into())
}

#[pin_data]
pub struct MessageInbox {
    messages: VecDeque<Message>,
    generation: u64,
    #[pin]
    wait_queue: PollCondVar,
}

type MessageInboxBox = Pin<KBox<MessageInbox>>;

impl MessageInbox {
    pub fn new() -> Result<MessageInboxBox> {
        KBox::pin_init(
            pin_init::pin_init!(Self {
                messages: VecDeque::new(),
                generation: 0,
                wait_queue <- kernel::new_poll_condvar!("motherboard-inbox-wait"),
            }),
            GFP_KERNEL,
        )
    }

    fn queue(self: Pin<&mut Self>, message: QueuedInboxMessage) {
        let this = self.project();
        this.messages.push_back(Message(message));
        *this.generation = this.generation.wrapping_add(1);
        this.wait_queue.notify_all();
    }

    fn bump(self: Pin<&mut Self>) {
        let this = self.project();
        *this.generation = this.generation.wrapping_add(1);
        this.wait_queue.notify_all();
    }

    fn try_pop(self: Pin<&mut Self>) -> Option<Message> {
        self.project().messages.pop_front()
    }

    fn generation(&self) -> u64 {
        self.generation
    }

    fn wait_queue(&self) -> &PollCondVar {
        &self.wait_queue
    }
}

pub struct Inboxes(HashMap<FileId, MessageInboxBox>);

impl Inboxes {
    pub fn new() -> Self {
        Self(HashMap::new())
    }

    fn ensure(&mut self, file: FileId) -> Result<&mut MessageInboxBox> {
        if !self.0.contains_key(&file) {
            self.0.insert(file, MessageInbox::new()?);
        }

        self.0.get_mut(&file).ok_or(ENOMEM)
    }

    pub fn queue(&mut self, file: FileId, message: QueuedInboxMessage) -> Result {
        self.ensure(file)?.as_mut().queue(message);
        Ok(())
    }

    pub fn bump(&mut self, file: FileId) -> Result {
        self.ensure(file)?.as_mut().bump();
        Ok(())
    }

    pub fn notify(&mut self, file: FileId) {
        if let Some(inbox) = self.0.get_mut(&file) {
            inbox.as_mut().bump();
        }
    }

    pub fn fetch(&mut self, file: FileId) -> Result<Option<InboxMessage>> {
        let Some(inbox) = self.0.get_mut(&file) else {
            return Ok(None);
        };
        let Some(message) = inbox.as_mut().try_pop() else {
            return Ok(None);
        };

        Ok(Some(message.0.into_user_message()?))
    }

    pub fn latch_generation(&mut self, file: FileId) -> Result<u64> {
        Ok(self.ensure(file)?.as_ref().get_ref().generation())
    }

    pub fn poll_latch(
        &self,
        file: FileId,
        observed_generation: u64,
        poll: FakeFilePoll<'_>,
    ) -> FakeFilePollEvents {
        let Some(inbox) = self.0.get(&file).map(|inbox| inbox.as_ref().get_ref()) else {
            return FakeFilePollEvents::HANG_UP;
        };

        if inbox.generation() != observed_generation {
            return FakeFilePollEvents::READ_READY;
        }

        poll.register_wait(inbox.wait_queue());

        if inbox.generation() != observed_generation {
            FakeFilePollEvents::READ_READY
        } else {
            FakeFilePollEvents::empty()
        }
    }
    pub fn broadcast(&mut self, message: QueuedInboxMessage) {
        for inbox in self.0.values_mut() {
            inbox.as_mut().queue(message.clone());
        }
    }
    pub fn close_inbox(&mut self, file: FileId) {
        self.0.remove(&file);
    }
}
