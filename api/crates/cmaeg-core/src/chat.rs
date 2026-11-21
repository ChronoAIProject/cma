//! Chat text conventions shared by routing and turn rendering.

mod address;

pub use address::{bot_address_len, leading_address_end, strip_address, strip_mention_keys};
