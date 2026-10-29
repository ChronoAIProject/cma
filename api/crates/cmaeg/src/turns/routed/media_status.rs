//! Retained metadata discovery does not acquire execution leases or expose text,
//! workspace paths, keys or access to an Agent's wider conversation history.
use super::{ExecutionError, MediaObligationStore};
use crate::ingress::routing_store::AdmittedPlan;
use serde::Serialize;
use utoipa::ToSchema;
#[derive(Serialize, ToSchema)]
pub struct MediaResponsePage {
    pub items: Vec<RetainedMediaResponse>,
    pub next_cursor: Option<usize>,
}
#[derive(Serialize, ToSchema)]
pub struct RetainedMediaResponse {
    pub consumer_id: String,
    pub attachment_index: u8,
    pub kind: String,
    pub response_id: Option<String>,
    pub response_url: Option<String>,
    pub status: String,
    pub delivery: String,
    pub refusal: Option<String>,
}
impl MediaObligationStore {
    pub async fn retained_responses(
        &self,
        plan: &AdmittedPlan,
        after: usize,
    ) -> Result<MediaResponsePage, ExecutionError> {
        let attachments = crate::turns::render_media::attachments(&plan.activity);
        let slots: Vec<_> = plan
            .targets
            .iter()
            .filter(|t| {
                t.launch.provider.slug.as_str() == "cma_codex"
                    && !t.launch.provider.target.is_profile()
            })
            .flat_map(|target| {
                attachments
                    .iter()
                    .enumerate()
                    .filter(|(_, a)| a.source.is_some())
                    .map(move |(index, _)| (target, index as u8))
            })
            .collect();
        if after > slots.len() {
            return Err(ExecutionError::Invalid);
        }
        let end = (after + 16).min(slots.len());
        let mut page = MediaResponsePage {
            items: Vec::new(),
            next_cursor: (end < slots.len()).then_some(end),
        };
        for (target, index) in &slots[after..end] {
            let Some(record) = self.load(plan, target, *index).await? else {
                continue;
            };
            if record.supplements.is_empty() {
                page.items.push(RetainedMediaResponse {
                    consumer_id: target.launch.consumer.to_string(),
                    attachment_index: *index,
                    kind: "original".into(),
                    response_id: None,
                    response_url: None,
                    status: if record.refusal.is_some() {
                        "refused"
                    } else {
                        "pending"
                    }
                    .into(),
                    delivery: "pending".into(),
                    refusal: record.refusal,
                });
                continue;
            }
            for supplement in &record.supplements {
                let response = if supplement.admission.is_some() {
                    self.retained_media_response(&record, supplement.artifact.kind)
                        .await?
                } else {
                    None
                };
                let response_id = supplement.admission.as_ref().map(|a| a.response_id.clone());
                page.items.push(RetainedMediaResponse {
                    consumer_id: target.launch.consumer.to_string(),
                    attachment_index: *index,
                    kind: supplement.artifact.kind.as_str().into(),
                    response_url: response_id
                        .as_ref()
                        .map(|id| format!("/api/v2/responses/{id}")),
                    response_id,
                    status: response
                        .as_ref()
                        .map(|r| r.status.clone())
                        .unwrap_or_else(|| {
                            if record.refusal.is_some() {
                                "refused"
                            } else {
                                "pending"
                            }
                            .into()
                        }),
                    delivery: response
                        .as_ref()
                        .map(super::media_delivery::summary)
                        .unwrap_or("pending")
                        .into(),
                    refusal: record.refusal.clone(),
                });
            }
            if let Some(kind) = record
                .pending_derivative
                .filter(|kind| !record.supplements.iter().any(|s| s.artifact.kind == *kind))
            {
                page.items.push(RetainedMediaResponse {
                    consumer_id: target.launch.consumer.to_string(),
                    attachment_index: *index,
                    kind: kind.as_str().into(),
                    response_id: None,
                    response_url: None,
                    status: if record.refusal.is_some() {
                        "refused"
                    } else {
                        "pending"
                    }
                    .into(),
                    delivery: "pending".into(),
                    refusal: record.refusal.clone(),
                });
            }
        }
        Ok(page)
    }
}
