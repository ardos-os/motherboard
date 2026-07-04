use std::{
    error::Error,
    os::fd::AsRawFd,
    time::{Duration, Instant},
};

use motherboard_client::{
    ClientApi, ClientCallsApi, ClientError, InboxMessage, MotherboardClient, ReplyStatus, RequestId,
};

const SERVICE_NAME: &str = "PingService";
const METHOD_NAME: &str = "ping";
const DEFAULT_ITERATIONS: usize = 10_000;
const DEFAULT_WARMUP_ITERATIONS: usize = 100;

fn main() -> Result<(), Box<dyn Error>> {
    let iterations = arg_usize(1).unwrap_or(DEFAULT_ITERATIONS);
    let warmup_iterations = arg_usize(2).unwrap_or(DEFAULT_WARMUP_ITERATIONS);
    let motherboard = MotherboardClient::open()?;

    for _ in 0..warmup_iterations {
        ping_once(&motherboard)?;
    }

    let mut samples = Vec::with_capacity(iterations);
    let started = Instant::now();
    for _ in 0..iterations {
        samples.push(ping_once(&motherboard)?);
    }
    let total = started.elapsed();

    samples.sort_unstable();
    print_summary(&samples, total, warmup_iterations);
    Ok(())
}

fn ping_once(motherboard: &MotherboardClient) -> Result<Duration, Box<dyn Error>> {
    let started = Instant::now();
    let request_id = motherboard.client().calls().call(
        SERVICE_NAME,
        METHOD_NAME,
        Box::<[u8]>::default(),
        Box::<[u32]>::default(),
    )?;

    wait_for_reply(motherboard, request_id)?;
    Ok(started.elapsed())
}

fn wait_for_reply(
    motherboard: &MotherboardClient,
    request_id: RequestId,
) -> Result<(), Box<dyn Error>> {
    loop {
        match motherboard.client().fetch() {
            Ok(InboxMessage::FunctionCallReply {
                request_id: reply_id,
                status: ReplyStatus::Ok,
                ..
            }) if reply_id == request_id => return Ok(()),
            Ok(InboxMessage::FunctionCallReply {
                request_id: reply_id,
                status: ReplyStatus::Error { code, message },
                payload,
                ..
            }) if reply_id == request_id => {
                return Err(format!(
                    "ping failed: code={code} message={message} payload={}",
                    String::from_utf8_lossy(&payload)
                )
                .into());
            }
            Ok(message) => {
                eprintln!("ignored inbox message while waiting for {request_id:?}: {message:?}");
            }
            Err(ClientError::WouldBlock(latch_fd)) => {
                wait_for_latch(&latch_fd)?;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

fn print_summary(samples: &[Duration], total: Duration, warmup_iterations: usize) {
    if samples.is_empty() {
        println!("no samples collected");
        return;
    }

    let total_sample_nanos: u128 = samples.iter().map(Duration::as_nanos).sum();
    let average = Duration::from_nanos((total_sample_nanos / samples.len() as u128) as u64);
    let throughput = samples.len() as f64 / total.as_secs_f64();

    println!("motherboard ping round-trip benchmark");
    println!("samples: {}", samples.len());
    println!("warmup: {warmup_iterations}");
    println!("total: {}", format_duration(total));
    println!("throughput: {throughput:.2} round-trips/sec");
    println!("min: {}", format_duration(samples[0]));
    println!("avg: {}", format_duration(average));
    println!("p50: {}", format_duration(percentile(samples, 50.0)));
    println!("p90: {}", format_duration(percentile(samples, 90.0)));
    println!("p99: {}", format_duration(percentile(samples, 99.0)));
    println!("max: {}", format_duration(samples[samples.len() - 1]));
}

fn percentile(samples: &[Duration], percentile: f64) -> Duration {
    let index = ((percentile / 100.0) * (samples.len().saturating_sub(1) as f64)).round() as usize;
    samples[index.min(samples.len() - 1)]
}

fn format_duration(duration: Duration) -> String {
    let nanos = duration.as_nanos();
    if nanos < 1_000 {
        format!("{nanos} ns")
    } else if nanos < 1_000_000 {
        format!("{:.2} us", nanos as f64 / 1_000.0)
    } else if nanos < 1_000_000_000 {
        format!("{:.2} ms", nanos as f64 / 1_000_000.0)
    } else {
        format!("{:.2} s", duration.as_secs_f64())
    }
}

fn arg_usize(index: usize) -> Option<usize> {
    std::env::args().nth(index)?.parse().ok()
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
