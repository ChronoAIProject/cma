//! Server-owned image/activity authority for explicit fleet migrations.
mod authority;
mod service;
pub use authority::*;
pub use service::*;

mod observation;
pub use observation::*;
#[cfg(test)]
mod tests;
