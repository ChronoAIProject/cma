//! Registered durable gate/admission contracts for segmented publication.
use super::*;
use crate::store::agent_log_segments as segments;

mod retained_assessment;
mod writer_gate_tests;
