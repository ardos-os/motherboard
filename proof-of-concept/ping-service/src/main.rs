use std::{error::Error, os::fd::AsRawFd};

use motherboard_client::{
    ClientError, InboxMessage, MotherboardClient, ReplyStatus, ServerApi, ServerCallsApi,
};

const SERVICE_NAME: &str = "PingService";
const METHOD_NAME: &str = "ping";

fn main() -> Result<(), Box<dyn Error>> {
    let motherboard = MotherboardClient::open()?;
    motherboard.server().bind_service(SERVICE_NAME)?;
    println!("{SERVICE_NAME} bound; replying to {METHOD_NAME}");

    loop {
        match motherboard.server().fetch() {
            Ok(InboxMessage::FunctionCallRequest {
                method,
                reply_token,
                payload,
                ..
            }) if method.as_str() == METHOD_NAME => {
                motherboard.server().calls().reply(
                    reply_token,
                    ReplyStatus::Ok,
                    payload,
                    Box::<[u32]>::default(),
                )?;
            }
            Ok(InboxMessage::FunctionCallRequest {
                method,
                reply_token,
                ..
            }) => {
                let message = format!("unknown method: {method}");
                motherboard.server().calls().reply(
                    reply_token,
                    ReplyStatus::Error {
                        code: "UnknownMethod".into(),
                        message: message.clone().into(),
                    },
                    message.into_bytes(),
                    Box::<[u32]>::default(),
                )?;
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
