use kernel::{alloc::KVec, ffi::c_int, fs::Kiocb, iov::IovIterDest, macros::vtable, prelude::*};

use super::{FakeFile, FakeFileFlags, create_fake_fd};

pub struct DataFd {
    bytes: KVec<u8>,
}

#[vtable]
impl FakeFile for DataFd {
    const NAME: &'static CStr = c"motherboard-data-fd";
    const FLAGS: FakeFileFlags = FakeFileFlags::READ_ONLY.union(FakeFileFlags::CLOSE_ON_EXEC);

    fn read_iter(
        this: &Self,
        iocb: &mut Kiocb<'_, KBox<Self>>,
        iter: &mut IovIterDest<'_>,
    ) -> Result<usize> {
        iter.simple_read_from_buffer(iocb.ki_pos_mut(), &this.bytes)
    }
}

impl DataFd {
    /// Creates a close-on-exec, read-only file descriptor containing `bytes`.
    ///
    /// Reading reaches EOF once all bytes have been consumed. Closing the descriptor releases the
    /// backing kernel allocation.
    pub fn wrap(bytes: KVec<u8>) -> Result<c_int> {
        create_fake_fd(DataFd { bytes })
    }
}
