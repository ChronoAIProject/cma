//! Shared SDK fixtures replayed against real in-memory REST composition.

use std::collections::BTreeMap;
use std::path::PathBuf;

use axum::Router;
use layer2_core::Authorization;
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod account_deletion_cases;
mod admission_control_cases;
mod agent_access_cases;
mod agent_cases;
mod agent_customization_cases;
mod agent_event_cases;
mod agent_restart_cases;
mod agent_worktree_recovery_cases;
mod artifact_cases;
mod attachment_cases;
mod authoring_cases;
mod binding_cases;
mod binding_media_cases;
mod bot_cases;
mod bot_setup_cases;
mod capacity_recovery_cases;
mod channel_usage_cases;
mod creation_inventory_cases;
mod creation_outcome_cases;
mod credential_cases;
mod deliverable_cases;
mod desktop_cases;
mod document_cases;
mod domains;
mod event_worker_cases;
mod experience_cases;
mod fault_cases;
mod filesystem_cases;
mod filesystem_runtime;
mod gateway_cases;
mod gateway_interaction_cases;
mod inventory;
mod inventory_cases;
mod legal_consent_cases;
mod lifecycle_cases;
mod lifecycle_result_contract;
mod message_feedback_cases;
mod metrics_cases;
mod platform_spend_cases;
mod provenance_cases;
mod provider_association_cases;
mod queued_cancellation_cases;
mod queued_input_cases;
mod replay;
mod resource_revocation_cases;
mod response_control_cases;
mod response_headers;
mod responses_v2_cases;
mod retained_clients;
mod retained_output_cases;
mod retained_recovery_cases;
mod runtime_canary_cases;
mod runtime_capacity_cases;
mod runtime_migration_cases;
mod runtime_update_cases;
mod sandbox_command_cases;
mod sandbox_inventory_cases;
mod share_cases;
mod share_link_cases;
mod shared_agent_cases;
mod shared_hierarchy_cases;
mod shared_source_cases;
mod shared_stream_cases;
mod source_cases;
mod state_cases;
mod suggestion_cases;
mod symbols;
mod task_experience_cases;
mod tenant_isolation;
mod trigger_cases;
mod usage_cases;
mod warm_pool_cases;
mod workspace_prepare_recovery_cases;

use symbols::Symbols;

pub(super) struct Scenario {
    pub router: Router,
    pub subject: Option<&'static str>,
    pub authorization: Authorization,
    pub symbols: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Corpus {
    version: u32,
    cases: Vec<Case>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Case {
    pub id: String,
    pub operation: String,
    pub scenario: String,
    pub request: RequestFixture,
    pub response: ResponseFixture,
    #[serde(default)]
    pub captures: BTreeMap<String, String>,
    #[serde(
        default,
        rename = "headerCaptures",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub header_captures: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RequestFixture {
    pub method: String,
    pub path: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
    #[serde(default, rename = "rawBody", skip_serializing_if = "Option::is_none")]
    pub raw_body: Option<String>,
    #[serde(
        default,
        rename = "rawBodyRepeat",
        skip_serializing_if = "Option::is_none"
    )]
    pub raw_body_repeat: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ResponseFixture {
    pub status: u16,
    #[serde(default, rename = "setCookies", skip_serializing_if = "Vec::is_empty")]
    pub set_cookies: Vec<String>,
    pub headers: BTreeMap<String, String>,
    pub body: Value,
    #[serde(rename = "bodyKind")]
    pub body_kind: BodyKind,
    #[serde(default, rename = "streamPrefix", skip_serializing_if = "is_false")]
    pub stream_prefix: bool,
}

fn is_false(value: &bool) -> bool {
    !value
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub(super) enum BodyKind {
    Json,
    Sse,
    Empty,
    Text,
}

fn directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/public-contract")
}

fn paths() -> Vec<PathBuf> {
    let mut paths: Vec<_> = std::fs::read_dir(directory())
        .expect("public contract fixtures exist")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|value| value == "json"))
        .collect();
    paths.sort();
    assert!(
        !paths.is_empty(),
        "public contract corpus must not be empty"
    );
    paths
}

async fn scenario(name: &str) -> Scenario {
    if let Some(scenario) = agent_restart_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = creation_outcome_cases::scenario(name) {
        return scenario;
    }
    if let Some(scenario) = super::admin::tests::binding_holds_tests::contract_scenario(name) {
        return scenario;
    }
    if let Some(scenario) = super::agent_original_request::tests::contract_scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = runtime_canary_cases::scenario(name) {
        return scenario;
    }
    if let Some(scenario) = Box::pin(runtime_migration_cases::scenario(name)).await {
        return scenario;
    }
    if let Some(scenario) = super::privacy_export::tests::contract_scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = resource_revocation_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = platform_spend_cases::scenario(name) {
        return scenario;
    }
    if let Some(scenario) = admission_control_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = account_deletion_cases::scenario(name) {
        return scenario;
    }
    if let Some(scenario) = legal_consent_cases::scenario(name) {
        return scenario;
    }
    if let Some(scenario) = capacity_recovery_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = task_experience_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = Box::pin(deliverable_cases::scenario(name)).await {
        return scenario;
    }
    if let Some(scenario) = metrics_cases::scenario(name) {
        return scenario;
    }
    if let Some(scenario) = usage_cases::scenario(name) {
        return scenario;
    }
    if let Some(scenario) = binding_media_cases::scenario(name) {
        return scenario;
    }
    if let Some(scenario) = agent_worktree_recovery_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = Box::pin(desktop_cases::scenario(name)).await {
        return scenario;
    }
    if let Some(scenario) = workspace_prepare_recovery_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = document_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = Box::pin(filesystem_cases::scenario(name)).await {
        return scenario;
    }
    if let Some(scenario) = Box::pin(share_link_cases::scenario(name)).await {
        return scenario;
    }
    if let Some(scenario) = message_feedback_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = suggestion_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = retained_recovery_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = runtime_update_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = attachment_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = agent_customization_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = agent_access_cases::scenario(name) {
        return scenario;
    }
    if let Some(scenario) = warm_pool_cases::scenario(name) {
        return scenario;
    }
    if let Some(scenario) = lifecycle_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = bot_setup_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = gateway_interaction_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = event_worker_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = bot_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = binding_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = queued_input_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = queued_cancellation_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = agent_event_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = response_control_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = provider_association_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = channel_usage_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = artifact_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = sandbox_command_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = creation_inventory_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = provenance_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = responses_v2_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = shared_stream_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = sandbox_inventory_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = shared_hierarchy_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = shared_source_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = inventory_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = shared_agent_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = share_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = source_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = credential_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = authoring_cases::scenario(name) {
        return scenario;
    }
    if let Some(scenario) = state_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = fault_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = gateway_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = agent_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = domains::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = experience_cases::scenario(name).await {
        return scenario;
    }
    if let Some(scenario) = retained_output_cases::scenario(name).await {
        return scenario;
    }
    trigger_cases::scenario(name)
        .await
        .unwrap_or_else(|| panic!("unknown contract scenario {name}"))
}

#[tokio::test]
async fn shared_public_contract_matches_real_backend() {
    run_contracts(false).await;
}

async fn run_contracts(isolation: bool) {
    let record = ["CMA_RECORD_PUBLIC_CONTRACT", "CMA_UPDATE_PUBLIC_CONTRACT"]
        .iter()
        .any(|key| std::env::var_os(key).is_some_and(|value| value == "1"));
    assert!(
        !isolation || !record,
        "isolation proof must compare immutable expected results"
    );
    let selected = std::env::var("CMA_PUBLIC_CONTRACT_FILE").ok();
    let mut total = 0;
    for path in paths() {
        if path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("auth-")
        {
            continue;
        }
        if selected
            .as_ref()
            .is_some_and(|selected| path.file_name().unwrap() != selected.as_str())
        {
            continue;
        }
        let mut corpus: Corpus = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(corpus.version, 1);
        if isolation && !corpus.cases.iter().any(tenant_isolation::included) {
            continue;
        }
        replay_corpus(&mut corpus, record, isolation).await;
        total += corpus.cases.len();
        if record {
            let mut encoded = serde_json::to_string_pretty(&corpus).unwrap();
            encoded.push('\n');
            std::fs::write(path, encoded).unwrap();
        }
    }
    assert!(total > 0, "no public contract cases selected");
    eprintln!("Replayed {total} shared public contract cases against real handlers");
}

async fn replay_corpus(corpus: &mut Corpus, record: bool, isolation: bool) {
    let mut canonical: BTreeMap<String, BTreeMap<String, Value>> = BTreeMap::new();
    for case in &corpus.cases {
        for (name, header) in &case.header_captures {
            let value = Value::String(
                case.response
                    .headers
                    .get(header)
                    .expect("declared header capture")
                    .clone(),
            );
            if let Some(previous) = canonical
                .entry(case.scenario.clone())
                .or_default()
                .insert(name.clone(), value.clone())
            {
                assert_eq!(
                    previous, value,
                    "conflicting canonical header capture {name}"
                );
            }
        }
        for (name, pointer) in &case.captures {
            let value = case
                .response
                .body
                .pointer(pointer)
                .unwrap_or_else(|| panic!("{} undeclared canonical capture {pointer}", case.id))
                .clone();
            if let Some(previous) = canonical
                .entry(case.scenario.clone())
                .or_default()
                .insert(name.clone(), value.clone())
            {
                assert_eq!(
                    previous, value,
                    "conflicting canonical declaration for {name}"
                );
            }
        }
    }
    let mut scenarios = BTreeMap::new();
    for case in &mut corpus.cases {
        if !scenarios.contains_key(&case.scenario) {
            let scene = scenario(&case.scenario).await;
            let mut symbols = Symbols::default();
            for (name, actual) in &scene.symbols {
                let canonical = canonical
                    .get(&case.scenario)
                    .and_then(|values| values.get(name))
                    .unwrap_or_else(|| panic!("undeclared seeded capture {name}"));
                symbols.seed(name, actual, canonical);
            }
            scenarios.insert(case.scenario.clone(), (scene, symbols));
        }
        let (scenario, symbols) = scenarios.get_mut(&case.scenario).unwrap();
        if isolation && tenant_isolation::selected(case) {
            tenant_isolation::probe(scenario, symbols, case).await;
        }
        replay::run(scenario, symbols, case, record).await;
    }
    for (_, symbols) in scenarios.values() {
        symbols.assert_complete();
    }
}
