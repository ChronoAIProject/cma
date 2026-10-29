//! Static structured content; advertised original-event reply destinations only.
use serde::{Deserialize, Serialize};

/// Exclusive neutral contact card, static location or venue payload.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum StructuredContent {
    /// A contact card, with no recipient authority.
    Contact {
        /// Wire field `phone_number`.
        phone_number: String,
        /// Wire field `first_name`.
        first_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Wire field `last_name`.
        last_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Wire field `vcard`.
        vcard: Option<String>,
    },
    /// A static point, never a live location.
    Location {
        /// Wire field `latitude`.
        latitude: f64,
        /// Wire field `longitude`.
        longitude: f64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Wire field `horizontal_accuracy`.
        horizontal_accuracy: Option<f64>,
    },
    /// Caller-supplied venue facts, without lookup or geocoding.
    Venue {
        /// Wire field `latitude`.
        latitude: f64,
        /// Wire field `longitude`.
        longitude: f64,
        /// Wire field `title`.
        title: String,
        /// Wire field `address`.
        address: String,
    },
}
