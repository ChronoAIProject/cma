//! Authorized Activity list response and name enrichment.
use super::*;
use layer2_core::{ActivityAction, ActivityQuery, ActivityRecord, ActivityTarget, DEFAULT_ACTIVITY_LIMIT};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActivityParams {
    actor: Option<String>,
    action: Option<ActivityAction>,
    limit: Option<u32>,
    cursor: Option<String>,
}

#[derive(Debug, Serialize, ToSchema)]
pub(crate) struct ActivityView {
    id: String,
    at: String,
    actor: Option<String>,
    actor_name: Option<String>,
    target_name: Option<String>,
    #[schema(value_type = String)]
    action: ActivityAction,
    #[schema(value_type = Option<Object>)]
    target: Option<ActivityTarget>,
    #[schema(value_type = Object)]
    outcome: layer2_core::ActivityOutcome,
    #[schema(value_type = Object)]
    detail: layer2_core::ActivityDetail,
}

impl From<ActivityRecord> for ActivityView {
    fn from(record: ActivityRecord) -> Self {
        Self {
            id: record.id.0,
            at: record.at,
            actor: record.actor.map(|actor| actor.0),
            actor_name: None,
            target_name: None,
            action: record.action,
            target: record.target,
            outcome: record.outcome,
            detail: record.detail,
        }
    }
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
        ("cursor" = Option<String>, Query)
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
    let page = state
        .activity
        .list(
            &caller,
            &ActivityQuery {
                actor: params.actor.map(SubjectId),
                action: params.action,
                limit: params.limit.unwrap_or(DEFAULT_ACTIVITY_LIMIT),
                cursor: params.cursor,
            },
        )
        .await?;
    let mut subjects: Vec<_> = page
        .items
        .iter()
        .filter_map(|row| row.actor.clone())
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
            .map(|row| {
                let mut view = ActivityView::from(row);
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

