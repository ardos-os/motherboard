use core::fmt::Display;

use alloc::string::String;
use kernel::{
    alloc::flags,
    fs::File,
    macros::vtable,
    miscdevice::{MiscDevice, MiscDeviceRegistration},
    prelude::*,
    uaccess::UserSlice,
};
use motherboardm_protocol::{ABI_VERSION, CommandEnvelope, MOTHERBOARD_IOCTL_EXECUTE, commands::*};

use crate::{
    fake_files::data_fd::DataFd,
    state::{AuthInfo, State},
};
use kernel::{bindings, current};
pub fn is_trusted_identity_context() -> bool {
    let current = current!();
    let user_ns_ok = {
        let current_ns = unsafe { bindings::current_user_ns() };
        let init_ns = core::ptr::addr_of_mut!(bindings::init_user_ns);
        current_ns == init_ns
    };

    let pid_ns_ok: bool = {
        let Some(pid_ns) = current.active_pid_ns() else {
            return false;
        };

        let init_ns = core::ptr::addr_of_mut!(bindings::init_pid_ns);
        pid_ns.as_ptr() == init_ns
    };

    user_ns_ok && pid_ns_ok
}

pub struct MotherboardDevice {
    pub auth_info: AuthInfo,
    pub file_id: FileId,
    pub cmdline: String,
}

#[vtable]
impl MiscDevice for MotherboardDevice {
    type Ptr = KBox<Self>;

    fn open(file: &File, _misc: &MiscDeviceRegistration<Self>) -> Result<Self::Ptr> {
        let current = kernel::current!();
        Ok(KBox::new(
            Self {
                auth_info: AuthInfo {
                    uid: current.uid().into_uid_in_current_ns(),
                    pid_with_ns: current.into(),
                    is_trusted: is_trusted_identity_context(),
                },
                file_id: FileId::from(file),
                cmdline: crate::utils::cmdline_for_task(current).ok_or(EINVAL)?,
            },
            GFP_KERNEL,
        )?)
    }

    fn ioctl(device: &Self, _file: &File, cmd: u32, arg: usize) -> Result<isize> {
        if cmd != MOTHERBOARD_IOCTL_EXECUTE {
            log::error!(
                "unknown command 0x{:08x}, expected 0x{:08x}\n",
                cmd,
                MOTHERBOARD_IOCTL_EXECUTE
            );
            return Err(ENOTTY);
        }

        let envelope_slice = UserSlice::new(
            UserPtr::from_addr(arg),
            core::mem::size_of::<CommandEnvelope>(),
        );
        let mut envelope_slice_reader = envelope_slice.reader();
        let envelope = envelope_slice_reader
            .read::<CommandEnvelope>()
            .map_err(|error| {
                log::error!(
                    "failed to read command envelope: errno={}\n",
                    error.to_errno()
                );
                error
            })?;

        if envelope.version != ABI_VERSION {
            log::error!(
                "unsupported ABI version {}, expected {}\n",
                envelope.version,
                ABI_VERSION
            );
            return Err(EINVAL);
        }

        log::debug!("command_size={}\n", envelope.data_len);

        let mut encoded_slice = KVec::new();
        UserSlice::new(UserPtr::from_addr(envelope.data_ptr), envelope.data_len)
            .read_all(&mut encoded_slice, flags::GFP_KERNEL)
            .map_err(|error| {
                log::error!(
                    "failed to copy {} command bytes from userspace: errno={}\n",
                    envelope.data_len,
                    error.to_errno()
                );
                error
            })?;

        let command = Command::deserialize_from_bytes(&encoded_slice).map_err(|error| {
            log::error!(
                "failed to deserialize {} command bytes: {:?}\n",
                encoded_slice.len(),
                error
            );
            EINVAL
        })?;
        let result = State::use_state(|s| {
            s.execute_command(command, device.auth_info, device.file_id, &device.cmdline)
        });
        let result_bytes: KVec<u8> = result.serialize_to_kvec().map_err(|error| {
            log::error!("failed to serialize result: {:?}\n", error);
            EINVAL
        })?;

        let result_size = result_bytes.len();
        let result_fd = DataFd::wrap(result_bytes).map_err(|error| {
            log::error!("failed to create response fd: errno={}\n", error.to_errno());
            error
        })?;

        log::debug!(
            "command completed, result_size={} result_fd={}\n",
            result_size,
            result_fd
        );
        Ok(result_fd as isize)
    }
}

impl MotherboardDevice {
    pub fn file_id(&self) -> FileId {
        self.file_id
    }
}
impl Drop for MotherboardDevice {
    fn drop(&mut self) {
        State::use_state(|s| s.cleanup(self))
    }
}

/// Represents an ID of a concrete File
///
/// This is NOT a file descriptor, this is analogous to an `Arc<File>`, file descriptors are clones of Arc
/// while this FileId uniquely represents the File inside.
#[derive(Hash, PartialEq, Eq, Copy, Clone, Debug)]
#[repr(transparent)]
pub struct FileId(*mut bindings::file);

// SAFETY: This type is only used as a identifier and never dereferenced
unsafe impl Send for FileId {}
unsafe impl Sync for FileId {}

impl<'a> From<&'a File> for FileId {
    fn from(value: &'a File) -> Self {
        Self(value.as_ptr())
    }
}
impl Display for FileId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_fmt(format_args!("{}", self.0 as usize))
    }
}
