//! The document every AG-UI state stream starts from.
//!
//! `STATE_SNAPSHOT` is deliberately not offered. A client instead creates
//! this document once per session and applies every `STATE_DELTA` to it in
//! order. Keeping the literal here makes the public documentation testable
//! byte for byte as well as structurally.

use serde_json::Value;

/// Every RFC 6901 path this build writes in an AG-UI `STATE_DELTA`.
///
/// The projection test derives the emitted set by exercising the durable and
/// live lanes, then compares it with this table. The documentation test pins
/// the public table to the same vocabulary.
#[cfg(test)]
pub(super) const STATE_DELTA_PATHS: &[&str] = &[
    "/availableCommands",
    "/configOptions",
    "/contextCompactions/-",
    "/gaps/-",
    "/memoryPressure",
    "/mode",
    "/plan",
    "/planeChanges/-",
    "/resources",
    "/runtimeLosses/-",
    "/signIn",
    "/usage/contextUsed",
    "/usage/contextWindow",
    "/usage/inputTokens",
    "/usage/outputTokens",
];

pub(super) const INITIAL_STATE_JSON: &str = r#"{
  "usage": {
    "inputTokens": null,
    "outputTokens": null,
    "contextUsed": null,
    "contextWindow": null
  },
  "mode": null,
  "plan": null,
  "configOptions": null,
  "availableCommands": null,
  "signIn": null,
  "gaps": [],
  "planeChanges": [],
  "contextCompactions": [],
  "runtimeLosses": [],
  "memoryPressure": null,
  "resources": null
}"#;

/// Return a fresh canonical document for an AG-UI session's state deltas.
pub fn initial_state() -> Value {
    serde_json::from_str(INITIAL_STATE_JSON)
        .expect("the compile-time AG-UI initial state must remain valid JSON")
}

#[cfg(test)]
#[path = "state_tests.rs"]
mod state_tests;
