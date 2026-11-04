use super::context::{Case, Context};
use cma_sdk::{
    Result,
    agents::{AnswerDeliverableReading, DeliverableResource},
};

fn metadata<'a>(case: &Case, corpus: &'a [Case]) -> &'a Case {
    let path = case.request.path.strip_suffix("/download").unwrap();
    let candidates: Vec<_> = corpus
        .iter()
        .filter(|entry| {
            entry.request.path == path
                && entry.response.status == 200
                && matches!(
                    entry.response.body["item"]["status"].as_str(),
                    Some("ready" | "unchecked" | "version_changed")
                )
        })
        .collect();
    candidates
        .iter()
        .copied()
        .find(|entry| entry.scenario == case.scenario)
        .or_else(|| {
            candidates
                .iter()
                .copied()
                .find(|entry| entry.scenario == "deliverable_owner")
        })
        .unwrap_or_else(|| panic!("{} missing real metadata companion", case.id))
}

pub fn exchange<'a>(case: &'a Case, corpus: &'a [Case]) -> Vec<&'a Case> {
    if !case.operation.contains("/deliverables/") || !case.operation.ends_with("/download") {
        return vec![case];
    }
    let before = metadata(case, corpus);
    if case.response.status < 400 {
        vec![before, case, before]
    } else {
        vec![before, case]
    }
}

pub async fn dispatch(context: &Context<'_>, corpus: &[Case]) -> Result<()> {
    let agents = context.client.agents();
    let token = context
        .case
        .request
        .headers
        .get("x-cma-document-share-token")
        .map(String::as_str);
    match context.case.operation.as_str() {
        "POST /api/v1/agents/{id}/deliverable-references" => {
            let resource: DeliverableResource =
                serde_json::from_value(context.case.request.body.clone().unwrap()).unwrap();
            context.check(
                agents
                    .describe_deliverable(context.param("id"), &resource, token)
                    .await,
            )
        }
        "GET /api/v1/agents/{id}/answers/{answer_id}/deliverables" => context.check(
            agents
                .answer_deliverables(context.param("id"), context.param("answer_id"), token)
                .await,
        ),
        "GET /api/v1/agents/{id}/answers/{answer_id}/deliverables/{delivery_id}" => context.check(
            agents
                .read_answer_deliverable(
                    context.param("id"),
                    context.param("answer_id"),
                    context.param("delivery_id"),
                    context
                        .query
                        .get("verify")
                        .is_some_and(|value| value == "true"),
                    token,
                )
                .await,
        ),
        "GET /api/v1/agents/{id}/answers/{answer_id}/deliverables/{delivery_id}/download" => {
            let expected: AnswerDeliverableReading =
                serde_json::from_value(metadata(context.case, corpus).response.body.clone())
                    .unwrap();
            let bytes = agents
                .download_answer_deliverable(
                    context.param("id"),
                    context.param("answer_id"),
                    &expected.item,
                    token,
                )
                .await?;
            context.check(Ok(String::from_utf8(bytes).unwrap()))
        }
        operation => panic!("unmapped deliverable operation {operation}"),
    }
}
