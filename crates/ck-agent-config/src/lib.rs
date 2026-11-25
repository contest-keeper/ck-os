//! Turns the admin's settings file into a configured contest machine at boot.
//!
//! Every path is resolved below a [`Root`]: `/` on the stick, a temporary folder in tests.

mod apply;
mod branding;
mod enrolment;
mod error;
mod load;
mod root;
mod trust;
mod usb_storage;

pub use apply::{
    BLOCK_BLUETOOTH, WIFI_CONNECTION, apply_basics, bluetooth_unblock_needed,
    bluetooth_unblock_pending, finish_bluetooth_unblock,
};
pub use branding::apply_branding;
pub use enrolment::{PendingEnrolment, read_applied_config};
pub use error::Error;
pub use load::{Loaded, load};
pub use root::Root;
pub use trust::{Booted, TRUST_MARKER, Trust, load_for_boot, load_last_good, write_trust};
pub use usb_storage::{BLOCK_USB_STORAGE, boot_usb_devices, usb_storage_blocked};
