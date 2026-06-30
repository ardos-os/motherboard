// kernel_logger.rs

use kernel::prelude::*;
use log::{Level, LevelFilter, Log, Metadata, Record};

pub struct KernelLogger;

static LOGGER: KernelLogger = KernelLogger;

impl Log for KernelLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= log::max_level()
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }

        let target = record.target();
        let args = record.args();

        match record.level() {
            Level::Error => {
                kernel::pr_err!("[{}] {}\n", target, args);
            }
            Level::Warn => {
                kernel::pr_warn!("[{}] {}\n", target, args);
            }
            Level::Info => {
                kernel::pr_info!("[{}] {}\n", target, args);
            }
            Level::Debug => {
                kernel::pr_debug!("[{}] {}\n", target, args);
            }
            Level::Trace => {
                // O kernel Rust talvez não tenha pr_trace!, dependendo da versão.
                // Podes mapear trace para debug.
                kernel::pr_debug!("[trace:{}] {}\n", target, args);
            }
        }
    }

    fn flush(&self) {
        // printk/pr_* não precisa de flush aqui.
    }
}

pub fn init(level: LevelFilter) -> Result {
    log::set_logger(&LOGGER).map_err(|_| EBUSY)?;
    log::set_max_level(level);
    Ok(())
}
