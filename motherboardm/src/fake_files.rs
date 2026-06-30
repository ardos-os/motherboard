pub mod data_fd;
pub mod inbox_latch;
use core::marker::PhantomData;

use kernel::{
    bindings,
    error::VTABLE_DEFAULT_ERROR,
    ffi::{CStr, c_int},
    fs::{File, Kiocb},
    iov::IovIterDest,
    macros::vtable,
    prelude::*,
    sync::poll::PollTable,
    types::ForeignOwnable,
};

bitflags::bitflags! {
    /// Flags passed to `anon_inode_getfd` for fake files.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct FakeFileFlags: u32 {
        /// Open the fake file read-only. This is Linux's zero-valued access mode.
        const READ_ONLY = bindings::O_RDONLY;
        const WRITE_ONLY = bindings::O_WRONLY;
        const READ_WRITE = bindings::O_RDWR;
        const CLOSE_ON_EXEC = bindings::O_CLOEXEC;
        const NONBLOCK = bindings::O_NONBLOCK;
    }
}

bitflags::bitflags! {
    /// Readiness events returned by fake file `poll` implementations.
    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    pub struct FakeFilePollEvents: bindings::__poll_t {
        const READABLE = bindings::POLLIN;
        const READ_NORMAL = bindings::POLLRDNORM;
        const WRITABLE = bindings::POLLOUT;
        const WRITE_NORMAL = bindings::POLLWRNORM;
        const ERROR = bindings::POLLERR;
        const HANG_UP = bindings::POLLHUP;
    }
}

impl FakeFileFlags {
    pub const fn as_raw(self) -> c_int {
        self.bits() as c_int
    }
}

impl FakeFilePollEvents {
    pub const READ_READY: Self = Self::READABLE.union(Self::READ_NORMAL);
    pub const WRITE_READY: Self = Self::WRITABLE.union(Self::WRITE_NORMAL);

    pub const fn as_raw(self) -> bindings::__poll_t {
        self.bits()
    }
}

/// Poll context passed to fake file poll implementations.
pub struct FakeFilePoll<'a> {
    file: &'a File,
    table: PollTable<'a>,
}

impl<'a> FakeFilePoll<'a> {
    fn new(file: &'a File, table: PollTable<'a>) -> Self {
        Self { file, table }
    }

    /// Registers this poll operation with a wait queue.
    ///
    /// Implementations should generally check readiness, register the wait queue when not ready,
    /// then check readiness again before returning empty events.
    pub fn register_wait(&self, cv: &kernel::sync::poll::PollCondVar) {
        self.table.register_wait(self.file, cv);
    }
}

#[vtable]
pub trait FakeFile: Sized + Send + Sync + 'static {
    const NAME: &'static CStr;
    const FLAGS: FakeFileFlags;

    fn read_iter(
        _this: &Self,
        _iocb: &mut Kiocb<'_, KBox<Self>>,
        _iter: &mut IovIterDest<'_>,
    ) -> Result<usize> {
        build_error!(VTABLE_DEFAULT_ERROR)
    }

    fn poll(_this: &Self, _poll: FakeFilePoll<'_>) -> FakeFilePollEvents {
        build_error!(VTABLE_DEFAULT_ERROR)
    }

    fn release(_this: KBox<Self>) {}
}

struct FakeFileVTable<T>(PhantomData<T>);

impl<T: FakeFile> FakeFileVTable<T> {
    unsafe extern "C" fn read_iter(
        raw_iocb: *mut bindings::kiocb,
        raw_iter: *mut bindings::iov_iter,
    ) -> isize {
        // SAFETY: The VFS invokes this callback only with an iocb for this vtable.
        let mut iocb = unsafe { Kiocb::<KBox<T>>::from_raw(raw_iocb) };
        // SAFETY: read_iter receives a destination iterator that is valid for this callback.
        let mut iter = unsafe { IovIterDest::from_raw(raw_iter) };
        let this = iocb.file();

        match T::read_iter(this, &mut iocb, &mut iter) {
            Ok(count) => count as isize,
            Err(error) => error.to_errno() as isize,
        }
    }

    unsafe extern "C" fn poll(
        raw_file: *mut bindings::file,
        raw_table: *mut bindings::poll_table,
    ) -> bindings::__poll_t {
        // SAFETY: The poll call of a file can access the private data.
        let private: *const _ = unsafe { (*raw_file).private_data };
        // SAFETY: The private pointer belongs to a T allocated by create_fake_fd and remains live
        // until release.
        let this = unsafe { &*private.cast::<T>() };
        // SAFETY: The VFS provides a valid file pointer for the duration of this callback.
        let file = unsafe { File::from_raw_file(raw_file) };
        // SAFETY: The VFS provides either a null or valid poll table for this callback.
        let table = unsafe { PollTable::from_raw(raw_table) };
        let poll = FakeFilePoll::new(file, table);

        T::poll(this, poll).as_raw()
    }

    unsafe extern "C" fn release(
        _inode: *mut bindings::inode,
        raw_file: *mut bindings::file,
    ) -> c_int {
        // SAFETY: anon_inode_getfd stored a pointer produced by KBox::into_foreign, and release is
        // called exactly once when the final reference to this struct file is dropped.
        let private = unsafe { (*raw_file).private_data };
        // SAFETY: This is the unique matching from_foreign call for the pointer above.
        let file = unsafe { KBox::<T>::from_foreign(private) };
        T::release(file);
        0
    }

    const VTABLE: bindings::file_operations = bindings::file_operations {
        owner: crate::THIS_MODULE.as_ptr(),
        read_iter: if T::HAS_READ_ITER {
            Some(Self::read_iter)
        } else {
            None
        },
        poll: if T::HAS_POLL { Some(Self::poll) } else { None },
        release: Some(Self::release),
        ..pin_init::zeroed()
    };

    const fn get() -> &'static bindings::file_operations {
        &Self::VTABLE
    }
}

pub fn create_fake_fd<T: FakeFile>(file: T) -> Result<c_int> {
    let file = KBox::new(file, GFP_KERNEL)?;
    let private = file.into_foreign();

    // SAFETY: The name and vtable are static, and `private` remains owned by the resulting file
    // until release. On failure, ownership is recovered below.
    let fd = unsafe {
        raw::anon_inode_getfd(
            T::NAME.as_char_ptr(),
            FakeFileVTable::<T>::get(),
            private,
            T::FLAGS.as_raw(),
        )
    };

    if fd < 0 {
        // SAFETY: anon_inode_getfd did not take ownership when it returned an error.
        drop(unsafe { KBox::<T>::from_foreign(private) });
        return Err(Error::from_errno(fd));
    }

    Ok(fd)
}

mod raw {
    use core::ffi::{c_int, c_void};
    use kernel::bindings;

    unsafe extern "C" {
        #[allow(improper_ctypes)]
        pub unsafe fn anon_inode_getfd(
            name: *const kernel::ffi::c_char,
            fops: *const bindings::file_operations,
            private: *mut c_void,
            flags: c_int,
        ) -> c_int;
    }
}
