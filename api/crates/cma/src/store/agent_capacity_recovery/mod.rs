//! Capacity-only recovery. Storage admission and runtime completion are separate
//! fenced operations; neither permits replaying original input.
pub(super) mod admission;
mod authority;
pub(super) mod records;
pub(super) mod source;
pub(super) mod status;
