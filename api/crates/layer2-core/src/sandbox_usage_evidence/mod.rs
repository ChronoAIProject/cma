//! Immutable allocated evidence derived only from committed usage intervals.
mod allocated;

pub use allocated::{project, MinuteFact, Projection, MAX_FACTS};

#[cfg(test)]
mod tests;
