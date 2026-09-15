//! Durable explicit Sandbox controls. Acceptance stores intent; a worker obtains
//! fresh authority before applying a lifecycle transition. Existing lifecycle
//! recovery then completes that committed transition without replaying admission.

mod model;
pub use model::*;

mod authority;
mod store;
pub use authority::*;
pub use store::*;

mod transition;
pub use transition::*;

mod service;
pub use service::*;

mod worker;
pub use worker::*;
