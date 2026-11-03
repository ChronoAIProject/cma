#![allow(dead_code)]
use serde::Serialize;
use utoipa::ToSchema;

#[derive(Serialize, ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum DeliverableResourceView {
    AgentArtifact { id: String },
    Document { id: String, revision: u64 },
    LiveFile { path: String },
}

#[derive(Serialize, ToSchema)]
pub(crate) struct DeliverableReferenceView {
    pub agent_id: String,
    pub resource: DeliverableResourceView,
    pub name: String,
    pub media_type: String,
    pub size_bytes: u64,
    pub sha256: String,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct DeliverableDescriptionView {
    pub reference: DeliverableReferenceView,
    pub marker: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DeliverableStatusView {
    Ready,
    Unchecked,
    Changed,
    VersionChanged,
    Missing,
    Denied,
    NotReady,
    TooLarge,
    Unavailable,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct AnswerDeliverableView {
    pub id: String,
    pub reference: DeliverableReferenceView,
    pub status: DeliverableStatusView,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct AnswerDeliverablesView {
    pub agent_id: String,
    pub answer_id: String,
    pub turn_sequence: u64,
    pub items: Vec<AnswerDeliverableView>,
}

#[derive(Serialize, ToSchema)]
pub(crate) struct AnswerDeliverableReadingView {
    pub agent_id: String,
    pub answer_id: String,
    pub turn_sequence: u64,
    pub item: AnswerDeliverableView,
}
