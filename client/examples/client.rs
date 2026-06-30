use std::{
    error::Error,
    fs::OpenOptions,
    io::{Seek, SeekFrom, Write},
    os::fd::AsRawFd,
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use motherboard_client::{ClientError, InboxMessage, MotherboardClient, ReplyStatus};

const SERVICE_NAME: &str = "EchoService";

fn main() -> Result<(), Box<dyn Error>> {
    let motherboard = MotherboardClient::open()?;
    let shared_file = create_example_file()?;

    let mut pending = vec![motherboard.call(
        SERVICE_NAME,
        "ReadFd",
        b"please read the attached fd".to_vec().into_boxed_slice(),
        vec![shared_file.as_raw_fd() as u32].into_boxed_slice(),
    )?];
    println!("submitted requests {pending:?}");

    loop {
        if pending.is_empty() {
            return Ok(());
        }
        match motherboard.fetch() {
            Ok(InboxMessage::CallReply {
                request_id: reply_id,
                status: ReplyStatus::Ok,
                payload,
                ..
            }) if pending.contains(&reply_id) => {
                pending.retain(|r| reply_id != *r);
                println!("server read: {}", String::from_utf8_lossy(&payload));
                dbg!(Instant::now());
            }
            Ok(InboxMessage::CallReply {
                request_id: reply_id,
                status: ReplyStatus::Error { code, message },
                payload,
                ..
            }) if pending.contains(&reply_id) => {
                pending.retain(|r| reply_id != *r);
                println!(
                    "error reply code={code:?} message={message:?}: {}",
                    String::from_utf8_lossy(&payload)
                );
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

fn create_example_file() -> std::io::Result<std::fs::File> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let path = std::env::temp_dir().join(format!(
        "motherboardm-fd-example-{}-{unique}.txt",
        std::process::id()
    ));

    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(&path)?;
    std::fs::remove_file(&path)?;

    file.write_all(b"hello through an installed file descriptor")?;
    file.seek(SeekFrom::Start(0))?;
    Ok(file)
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
