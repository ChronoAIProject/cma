//! Native Discord transport only. No relay impersonation or production wiring.
//! Source credential custody, bot ownership, durable admission and REST delivery
//! must be integrated before a native Discord source can be advertised.
mod gateway;
mod protocol;

pub use gateway::GatewayClient;
pub use protocol::{
    Checkpoint, Dispatch, EndReason, GatewayEnd, GatewayError, NativeDiscordJournal,
    MESSAGE_INTENTS,
};

#[cfg(test)]
mod tests;
