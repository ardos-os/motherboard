use std::{env, fs, path::PathBuf, process::ExitCode};

use midl::RustMode;

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let mut args = env::args_os().skip(1);
    let input = args
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| usage("missing input file"))?;

    let mut rust_out = None;
    let mut mode = RustMode::Both;
    while let Some(arg) = args.next() {
        if arg == "--rust-out" {
            rust_out = Some(
                args.next()
                    .map(PathBuf::from)
                    .ok_or_else(|| usage("missing path after --rust-out"))?,
            );
        } else if arg == "--mode" {
            let raw_mode = args
                .next()
                .ok_or_else(|| usage("missing value after --mode"))?;
            mode = parse_mode(&raw_mode.to_string_lossy())?;
        } else {
            return Err(usage(format!(
                "unknown argument `{}`",
                arg.to_string_lossy()
            )));
        }
    }

    let source = fs::read_to_string(&input)
        .map_err(|error| format!("failed to read {}: {error}", input.display()))?;
    let document = midl::parse_document(&source)
        .map_err(|error| format!("failed to parse {}: {error}", input.display()))?;
    let rust = midl::generate_rust_with_mode(&document, mode);

    if let Some(path) = rust_out {
        fs::write(&path, rust)
            .map_err(|error| format!("failed to write {}: {error}", path.display()))?;
    } else {
        print!("{rust}");
    }

    Ok(())
}

fn usage(message: impl AsRef<str>) -> String {
    format!(
        "{}\nusage: midl <input.midl> [--mode client|server|both] [--rust-out <bindings.rs>]",
        message.as_ref()
    )
}

fn parse_mode(mode: &str) -> Result<RustMode, String> {
    match mode {
        "client" => Ok(RustMode::Client),
        "server" => Ok(RustMode::Server),
        "both" => Ok(RustMode::Both),
        _ => Err(usage(format!("unknown mode `{mode}`"))),
    }
}
