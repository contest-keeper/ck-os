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

pub use apply::{BLOCK_BLUETOOTH, WIFI_CONNECTION, apply_basics};
pub use branding::apply_branding;
pub use enrolment::{PendingEnrolment, read_applied_config};
pub use error::Error;
pub use load::{Loaded, Source, load, read_with_esp_fallback};
pub use root::Root;
pub use trust::{Booted, TRUST_MARKER, Trust, load_for_boot, load_last_good, write_trust};
