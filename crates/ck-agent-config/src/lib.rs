//! Turns the admin's settings file into a configured contest machine at boot.
//!
//! Every path is resolved below a [`Root`]: `/` on the stick, a temporary folder in tests.

mod apply;
mod branding;
mod error;
mod load;
mod root;

pub use apply::apply_basics;
pub use branding::apply_branding;
pub use error::Error;
pub use load::{Loaded, load};
pub use root::Root;
