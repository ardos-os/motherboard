use std::{
    error::Error,
    fs::File,
    io::Read,
    os::fd::{AsRawFd, FromRawFd},
    sync::Arc,
    time::Instant,
};

use motherboard_client::{ClientError, InboxMessage, MotherboardClient, ReplyStatus};

const SERVICE_NAME: &str = "EchoService";

fn main() -> Result<(), Box<dyn Error>> {
    let motherboard = Arc::new(MotherboardClient::open()?);
    motherboard.bind_service(SERVICE_NAME)?;
    println!("{SERVICE_NAME} bound; waiting for requests");

    loop {
        match motherboard.fetch() {
            Ok(InboxMessage::CallRequest {
                method,
                reply_token,
                payload,
                fds,
                origin,
                ..
            }) => {
                let motherboard = motherboard.clone();
                println!(
                    "request from pid={} uid={} method={method} payload={} bytes fds={}",
                    origin.pid,
                    origin.uid,
                    payload.len(),
                    fds.len()
                );

                let reply_payload = read_first_fd(&fds).unwrap_or_else(|error| {
                    format!("failed to read attached fd: {error}").into_bytes()
                });

                motherboard
                    .reply(
                        reply_token,
                        ReplyStatus::Ok,
                        reply_payload,
                        Box::<[u32]>::default(),
                    )
                    .unwrap();
                dbg!(Instant::now());
            }
            Ok(message) => {
                println!("ignored inbox message: {message:?}");
            }
            Err(ClientError::WouldBlock(latch_fd)) => {
                wait_for_latch(&latch_fd)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn read_first_fd(fds: &[u32]) -> std::io::Result<Vec<u8>> {
    let Some(fd) = fds.first() else {
        return Ok(b"request did not include any file descriptors".to_vec());
    };

    // SAFETY: motherboardm installed this descriptor into this process and transferred ownership
    // through the inbox message. Dropping File closes only this received descriptor.
    let mut file = unsafe { File::from_raw_fd(*fd as i32) };
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn wait_for_latch(latch_fd: &impl AsRawFd) -> std::io::Result<()> {
    let mut poll_fd = libc::pollfd {
        fd: latch_fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };

    loop {
        let result = unsafe { libc::poll(&mut poll_fd, 1, -1) };
        if result >= 0 {
            return Ok(());
        }

        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}
