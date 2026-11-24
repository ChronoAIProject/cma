//! Authorized Activity list response and name enrichment.
use super::*;
use layer2_core::{
    ActivityAction, ActivityQuery, ActivityRecord, ActivityTarget, DEFAULT_ACTIVITY_LIMIT,
};
use sha2::{Digest, Sha256};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActivityParams {
    actor: Option<String>,
    action: Option<ActivityAction>,
    limit: Option<u32>,
    cursor: Option<String>,
    view: Option<ListView>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum ListView {
    Summary,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ActivityView {
    id: String,
    at: String,
    actor: Option<String>,
    actor_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    actor_reference: Option<String>,
    target_name: Option<String>,
    #[schema(value_type = String)]
    action: ActivityAction,
    #[schema(value_type = Option<Object>)]
    target: Option<ActivityTarget>,
    #[schema(value_type = Object)]
    outcome: layer2_core::ActivityOutcome,
    #[schema(value_type = Object)]
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<layer2_core::ActivityDetail>,
}

impl From<ActivityRecord> for ActivityView {
    fn from(record: ActivityRecord) -> Self {
        Self {
            id: record.id.0,
            at: record.at,
            actor: record.actor.map(|actor| actor.0),
            actor_name: None,
            actor_reference: None,
            target_name: None,
            action: record.action,
            target: record.target,
            outcome: record.outcome,
            detail: Some(record.detail),
        }
    }
}

impl From<layer2_core::ActivitySummary> for ActivityView {
    fn from(record: layer2_core::ActivitySummary) -> Self {
        Self {
            id: record.id.0,
            at: record.at,
            actor: record.actor.map(|actor| actor.0),
            actor_name: None,
            actor_reference: None,
            target_name: None,
            action: record.action,
            target: record.target,
            outcome: record.outcome,
            detail: None,
        }
    }
}

fn actor_reference(actor: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(b"cma:operator-activity:actor:v1\0");
    digest.update(actor.as_bytes());
    let digest = format!("{:x}", digest.finalize());
    format!("actor-{}", &digest[..16])
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ActivityPageView {
    items: Vec<ActivityView>,
    next: Option<String>,
}

#[utoipa::path(
    get,
    path = "/api/v1/admin/activity",
    tag = "administration",
    params(
        ("actor" = Option<String>, Query),
        ("action" = Option<String>, Query),
        ("limit" = Option<u32>, Query),
        ("cursor" = Option<String>, Query),
        ("view" = Option<String>, Query, description = "summary omits undisplayed audit detail; absent preserves full records")
    ),
    responses(
        (status = 200, body = ActivityPageView),
        (status = 403, body = crate::rest::ProblemDocument)
    )
)]
pub(crate) async fn activity(
    State(state): State<Arc<AdminState>>,
    AuthenticatedCaller(caller): AuthenticatedCaller,
    params: Result<Query<ActivityParams>, axum::extract::rejection::QueryRejection>,
) -> Result<Json<ActivityPageView>, CollectionProblem> {
    let params = params.map_err(|_| CollectionProblem::InvalidRequest)?.0;
    let query = ActivityQuery {
        actor: params.actor.map(SubjectId),
        action: params.action,
        limit: params.limit.unwrap_or(DEFAULT_ACTIVITY_LIMIT),
        cursor: params.cursor,
    };
    let page = match params.view {
        Some(ListView::Summary) => {
            let page = state.activity.list_summaries(&caller, &query).await?;
            layer2_core::Page::new(
                page.items
                    .into_iter()
                    .map(ActivityView::from)
                    .collect::<Vec<_>>(),
                page.next_cursor,
            )
        }
        None => {
            let page = state.activity.list(&caller, &query).await?;
            layer2_core::Page::new(
                page.items
                    .into_iter()
                    .map(ActivityView::from)
                    .collect::<Vec<_>>(),
                page.next_cursor,
            )
        }
    };
    let mut subjects: Vec<_> = page
        .items
        .iter()
        .filter_map(|row| row.actor.clone().map(SubjectId))
        .collect();
    subjects.extend(page.items.iter().filter_map(|row| match &row.target {
        Some(ActivityTarget::Subject(subject)) => Some(subject.clone()),
        _ => None,
    }));
    subjects.sort_by(|a, b| a.0.cmp(&b.0));
    subjects.dedup();
    let mut names = std::collections::BTreeMap::new();
    for batch in subjects.chunks(200) {
        names.extend(crate::rest::public_names::resolve(&state.users, batch).await);
    }
    Ok(Json(ActivityPageView {
        items: page
            .items
            .into_iter()
            .map(|mut view| {
                view.actor_reference = view.actor.as_deref().map(actor_reference);
                view.actor_name = view
                    .actor
                    .as_ref()
                    .and_then(|actor| names.get(actor))
                    .cloned();
                view.target_name = match &view.target {
                    Some(ActivityTarget::Subject(subject)) => names.get(&subject.0).cloned(),
                    _ => None,
                };
                view
            })
            .collect(),
        next: page.next_cursor,
    }))
}
