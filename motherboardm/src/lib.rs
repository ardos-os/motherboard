//! Ardos Motherboard out-of-tree Linux module.
#![cfg_attr(cargo_nok_ra, no_std)]

use alloc::sync::Arc;
use kernel::macros::module;
extern crate alloc;
#[cfg(not(MODULE))]
compile_error!(
    "Must be compiled using cargo-nok, normal `cargo check` or `cargo build` will not work."
);
pub mod allocator_adapter;
pub mod fake_files;
pub mod logger;
pub mod module;
pub mod motherboard_device;
pub mod services;
pub mod state;
pub mod utils;
pub type SharedData = Arc<[u8]>;
pub type SharedStr = motherboardm_protocol::Str;

module! {
    type: module::MotherboardModule,
    name: "motherboardm",
    authors: ["coffeeispower"],
    description: "Ardos service locator kernel module",
    license: "GPL",
}
