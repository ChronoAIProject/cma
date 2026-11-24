//! Persistence for Agent Manager's bounded reporting read model.

mod store {
    pub(super) mod usage_reports;
}

pub use store::usage_reports::{
    chart_documents, decode, encode, expires, read_series, unavailable,
};
