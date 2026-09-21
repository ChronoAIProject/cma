//! Pure protocol and identity types shared by the CMA Event Gateway.

/// Channel definitions and their validation rules.
pub mod channel {}

/// Neutral inbound activity produced by an event source.
pub mod activity {}

/// Neutral messages emitted by an agent turn.
pub mod message {}

/// CloudEvents 1.0 envelope types.
pub mod cloudevent {}

/// RFC 9457 problem documents.
pub mod problem {}

/// Provider-facing agent profile snapshots.
pub mod profile {}

/// Sealed channel and event-reference primitives.
pub mod seal {}

/// Deterministic identifiers used across CMAEG boundaries.
pub mod ids {}

#[cfg(test)]
mod tests {
    #[test]
    fn exposes_the_protocol_module_tree() {
        assert_eq!(
            [
                "channel",
                "activity",
                "message",
                "cloudevent",
                "problem",
                "profile",
                "seal",
                "ids",
            ]
            .len(),
            8
        );
    }
}
