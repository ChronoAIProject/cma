use super::{digest, Census};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;

type Counts = BTreeMap<String, usize>;

// Only fixed domain classification labels enter the summary. No member, revision,
// source document, diagnostic detail or input payload is emitted here.
#[derive(PartialEq, Eq, PartialOrd, Ord, Serialize)]
struct Cohort {
    cause: String,
    agent_state: String,
    parent_state: String,
    recovery_route: String,
}

fn label(value: impl Serialize) -> String {
    serde_json::to_value(value)
        .expect("fixed enum")
        .as_str()
        .expect("fixed enum")
        .into()
}

fn increment(counts: &mut Counts, key: &str) {
    *counts.entry(key.into()).or_default() += 1;
}

pub(super) fn summary(census: &Census, bytes: &[u8]) -> Value {
    let mut outcomes = Counts::new();
    let mut causes = Counts::new();
    let mut routes = Counts::new();
    let mut parent_states = Counts::new();
    let mut agent_states = Counts::new();
    let mut preservation = Counts::new();
    let mut cohorts = BTreeMap::<Cohort, usize>::new();
    let mut evidence = Counts::new();
    let mut unknown = 0;
    for row in &census.rows {
        increment(&mut outcomes, row.outcome);
        let Some(assessment) = &row.assessment else {
            unknown += 1;
            increment(&mut preservation, "unresolved_member_preserve_all");
            continue;
        };
        let cohort = Cohort {
            cause: label(assessment.cause),
            agent_state: label(assessment.agent_state),
            parent_state: label(assessment.parent_state),
            recovery_route: assessment.recovery_route.clone(),
        };
        increment(&mut causes, &cohort.cause);
        increment(&mut routes, &cohort.recovery_route);
        increment(&mut parent_states, &cohort.parent_state);
        increment(&mut agent_states, &cohort.agent_state);
        *cohorts.entry(cohort).or_default() += 1;
        for requirement in &assessment.preserve {
            increment(&mut preservation, requirement);
        }
        for (name, present) in [
            ("cleanup_scheduled", assessment.cleanup_scheduled),
            ("cleanup_debt_present", assessment.cleanup_debt > 0),
            ("lease_present", assessment.lease_present),
            ("conversation_present", assessment.conversation_present),
            ("input_state_present", assessment.input_state_present),
            (
                "attempted_or_recorded_work",
                assessment.attempted_or_recorded_work,
            ),
            ("retained_home_present", assessment.retained_home_present),
        ] {
            if present {
                increment(&mut evidence, name);
            }
        }
    }
    let cohorts: Vec<_> = cohorts
        .into_iter()
        .map(|(classification, count)| json!({"classification":classification,"count":count}))
        .collect();
    json!({"format":"runtime-ledger-census-summary-v1",
        "snapshot_sha256":digest(bytes), "request_sha256":census.request_sha256,
        "source_sha256":census.source_sha256, "requested":census.rows.len(),
        "assessed":census.rows.len() - unknown, "unknown":unknown,
        "outcomes":outcomes,"causes":causes,"routes":routes,
        "parent_states":parent_states,"agent_states":agent_states,
        "cohorts":cohorts,"preservation":preservation,"evidence_counts":evidence,
        "writes":false, "recovery_authorized":false})
}
