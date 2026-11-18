use crate::activity::ChannelActivity;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Clone, Default, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in=Query)]
pub struct HistoryQuery {
    pub from: Option<String>,
    pub until: Option<String>,
    pub limit: Option<u16>,
    pub cursor: Option<String>,
}
#[derive(Clone, Default, Deserialize, IntoParams)]
#[serde(deny_unknown_fields)]
#[into_params(parameter_in=Query)]
pub struct DetailsQuery {
    pub cursor: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct HistoryRange {
    pub from: String,
    pub until: String,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CoverageGap {
    LegacyPartial,
    RetentionExpired,
    UnreadableRecord,
    CaptureGap,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct HistoryCoverage {
    pub complete_event_enumeration: bool,
    pub gaps: Vec<CoverageGap>,
    pub retained_from: String,
    pub all_writers_capture_from: Option<String>,
    pub all_writers_capture_until: Option<String>,
    /// Summaries may advance during paging; intermediate transitions are not archived.
    pub current_summaries: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct HistoryItem {
    /// Opaque identity used by the details route; activity.event_id is the source identity.
    pub history_event_id: String,
    pub activity: ChannelActivity,
    pub detail_cursor: Option<String>,
    pub detail_incomplete: bool,
    pub detail_truncated: bool,
    pub legacy_partial: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct HistoryPage {
    pub items: Vec<HistoryItem>,
    pub next_cursor: Option<String>,
    pub has_more: bool,
    pub range: HistoryRange,
    pub coverage: HistoryCoverage,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct HistoryDetails {
    pub generation: String,
    pub encoding: String,
    /// Concatenate decoded parts in index order, then deserialize ChannelActivity.
    pub parts: Vec<HistoryDetailPart>,
    pub next_cursor: Option<String>,
    pub detail_incomplete: bool,
    pub detail_truncated: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize, ToSchema)]
pub struct HistoryDetailPart {
    pub index: usize,
    pub sha256: String,
    pub data: String,
}
