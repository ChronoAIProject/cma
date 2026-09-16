//! Protocol fixtures use genuine RSA-signed NyxID callback tokens and a
//! disposable synthetic live-binding observation. These are not live receipts.
use super::*;
use layer2_core::bridge::ingress::*;
use layer2_core::bridge::*;
use serde_json::json;

mod additional_platforms;
mod controls;
pub(crate) mod event_cases;
mod event_matrix;
pub(in crate::bridge::ingress) mod fixtures;
mod normalization;
mod persistence;
mod refusals;

use fixtures::*;
