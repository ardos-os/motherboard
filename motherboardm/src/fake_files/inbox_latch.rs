use kernel::{ffi::c_int, macros::vtable, prelude::*};
use motherboardm_protocol::RawFd;

use crate::{
    fake_files::{FakeFile, FakeFileFlags, FakeFilePoll, FakeFilePollEvents, create_fake_fd},
    motherboard_device::FileId,
    state::State,
};

pub struct InboxLatch {
    file_id: FileId,
    observed_generation: u64,
}

#[vtable]
impl FakeFile for InboxLatch {
    const NAME: &'static CStr = c"motherboard-inbox-latch";
    const FLAGS: FakeFileFlags = FakeFileFlags::READ_ONLY
        .union(FakeFileFlags::CLOSE_ON_EXEC)
        .union(FakeFileFlags::NONBLOCK);

    fn poll(this: &Self, poll: FakeFilePoll<'_>) -> FakeFilePollEvents {
        State::use_state(|state| {
            state
                .inboxes()
                .poll_latch(this.file_id, this.observed_generation, poll)
        })
    }
}

impl InboxLatch {
    pub fn new(file_id: FileId, observed_generation: u64) -> Result<RawFd> {
        let fd: c_int = create_fake_fd(Self {
            file_id,
            observed_generation,
        })?;
        Ok(fd as RawFd)
    }
}
