//! Resource-neutral durable event vocabulary.

use serde::{Deserialize, Serialize};

/// A durable state edge without assumptions about the replacement aggregates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum EventKind {
    Created,
    Updated,
    Deleted,
    PressureOpened {
        dimension: PressureDimension,
        occupancy_permille: u16,
    },
    PressureClosed {
        dimension: PressureDimension,
        peak_permille: u16,
        duration_seconds: u32,
    },
}

/// Resource dimension carried by a pressure edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PressureDimension {
    Cpu,
    Memory,
}
