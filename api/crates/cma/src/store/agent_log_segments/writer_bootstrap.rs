//! Opaque proof of a complete, fenced staged legacy evidence generation.
//!
//! Captured counters alone cannot construct this proof. The executable staging
//! path will validate source coverage and its preparation lease before making a
//! plan available. This signature checkpoint provides no construction path.

pub(in crate::store) struct BootstrapPlan {
    _private: (),
}
