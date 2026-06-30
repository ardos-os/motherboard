use kernel::{
    bindings,
    error::to_result,
    miscdevice::{MiscDevice, MiscDeviceOptions},
    prelude::*,
    types::Opaque,
};

use crate::{logger, state::State};

const SERVICES_DEVICE_MODE: u16 = 0o666;

#[pin_data(PinnedDrop)]
pub struct MotherboardModule {
    #[pin]
    _device: PublicMiscDeviceRegistration<crate::motherboard_device::MotherboardDevice>,
}

impl kernel::InPlaceModule for MotherboardModule {
    fn init(_module: &'static ThisModule) -> impl PinInit<Self, Error> {
        logger::init(log::LevelFilter::Debug).ok();
        log::info!("motherboardm: loaded\n");
        try_pin_init!(Self {
            _device <- PublicMiscDeviceRegistration::register(c"services"),
        })
    }
}

#[pinned_drop]
impl PinnedDrop for MotherboardModule {
    fn drop(self: Pin<&mut Self>) {
        State::drop();
    }
}

#[repr(transparent)]
#[pin_data(PinnedDrop)]
struct PublicMiscDeviceRegistration<T> {
    #[pin]
    inner: Opaque<bindings::miscdevice>,
    _t: core::marker::PhantomData<T>,
}

// SAFETY: This mirrors the upstream Rust miscdevice registration wrapper. The C miscdevice can be
// deregistered from a different thread than the one that registered it.
unsafe impl<T> Send for PublicMiscDeviceRegistration<T> {}

// SAFETY: Shared access only exposes the pinned raw registration for deregistration during drop.
unsafe impl<T> Sync for PublicMiscDeviceRegistration<T> {}

impl<T: MiscDevice> PublicMiscDeviceRegistration<T> {
    fn register(name: &'static CStr) -> impl PinInit<Self, Error> {
        try_pin_init!(Self {
            inner <- Opaque::try_ffi_init(move |slot: *mut bindings::miscdevice| {
                let mut raw = MiscDeviceOptions { name }.into_raw::<T>();
                raw.mode = SERVICES_DEVICE_MODE;

                // SAFETY: The initializer can write to the provided slot before registration.
                unsafe { slot.write(raw) };

                // SAFETY: The slot now contains a fully initialized miscdevice. It remains pinned
                // and registered until this registration is dropped.
                to_result(unsafe { bindings::misc_register(slot) })
            }),
            _t: core::marker::PhantomData,
        })
    }
}

#[pinned_drop]
impl<T> PinnedDrop for PublicMiscDeviceRegistration<T> {
    fn drop(self: Pin<&mut Self>) {
        // SAFETY: Successful initialization registered this miscdevice exactly once.
        unsafe { bindings::misc_deregister(self.inner.get()) };
    }
}
