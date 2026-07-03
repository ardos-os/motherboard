use std::{error::Error, os::fd::AsRawFd};

use motherboard_client::{
    ClientError, InboxMessage, MotherboardClient, ReplyStatus, ServerApi, ServerCallsApi,
    ServerStoresApi,
};

const SERVICE_NAME: &str = "SettingsManager";
const THEME_STORE: &str = "theme";
const LIGHT_THEME: &[u8] = b"light";
const DARK_THEME: &[u8] = b"dark";

fn main() -> Result<(), Box<dyn Error>> {
    let motherboard = MotherboardClient::open()?;
    motherboard.server().bind_service(SERVICE_NAME)?;
    motherboard
        .server()
        .stores()
        .create(SERVICE_NAME, THEME_STORE, LIGHT_THEME.to_vec(), true)?;

    println!("{SERVICE_NAME} running; {THEME_STORE}=light");

    loop {
        match motherboard.server().fetch() {
            Ok(InboxMessage::FunctionCallRequest {
                method,
                reply_token,
                payload,
                origin,
                ..
            }) if method.as_ref() == "setTheme" => {
                let result = set_theme(&motherboard, &payload);
                match result {
                    Ok(theme) => {
                        println!(
                            "theme set to {theme} by pid={} uid={}",
                            origin.pid, origin.uid
                        );
                        motherboard.server().calls().reply(
                            reply_token,
                            ReplyStatus::Ok,
                            theme.into_bytes(),
                            Box::<[u32]>::default(),
                        )?;
                    }
                    Err(message) => {
                        motherboard.server().calls().reply(
                            reply_token,
                            ReplyStatus::Error {
                                code: "InvalidTheme".into(),
                                message: message.clone().into_boxed_str(),
                            },
                            message.into_bytes(),
                            Box::<[u32]>::default(),
                        )?;
                    }
                }
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
                        message: message.clone().into_boxed_str(),
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

fn set_theme(motherboard: &MotherboardClient, payload: &[u8]) -> Result<String, String> {
    let theme = std::str::from_utf8(payload)
        .map_err(|_| "theme payload must be valid UTF-8".to_string())?;

    let value = match theme {
        "light" => LIGHT_THEME,
        "dark" => DARK_THEME,
        _ => return Err(format!("unsupported theme: {theme}")),
    };

    motherboard
        .server()
        .stores()
        .update(SERVICE_NAME, THEME_STORE, value.to_vec())
        .map_err(|error| error.to_string())?;

    Ok(theme.to_string())
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
