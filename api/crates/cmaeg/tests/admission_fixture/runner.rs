use super::{config::local_origin, ledger::hash, Shared};
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

#[path = "runner_poll.rs"]
mod poll;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Run {
    pub case_id: String,
    pub event: String,
    pub endpoints: Vec<String>,
    #[serde(default = "one")]
    pub replays_per_ingress: usize,
}
fn one() -> usize {
    1
}
impl Run {
    fn validate(&self) -> Result<(), &'static str> {
        let id = uuid::Uuid::parse_str(&self.case_id).map_err(|_| "case identity")?;
        if id.get_version_num() != 4 || id.to_string() != self.case_id {
            return Err("case identity");
        }
        if !(1..=2).contains(&self.replays_per_ingress) {
            return Err("replay count");
        }
        if self.endpoints.is_empty() || self.endpoints.len() > 2 {
            return Err("endpoint count");
        }
        let canonical = self
            .endpoints
            .iter()
            .map(|endpoint| local_origin(endpoint, false).map(|url| url.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        if canonical.len() == 2 && canonical[0] == canonical[1] {
            return Err("N2 needs distinct direct ingresses");
        }
        Ok(())
    }
}

pub async fn run(shared: Arc<Shared>, input: Run) -> Result<Value, &'static str> {
    input.validate()?;
    // Match the generated diagnostic patch's canonical UUID-byte digest.
    let case_uuid = uuid::Uuid::parse_str(&input.case_id).map_err(|_| "case identity")?;
    let case_sha256 = hash(case_uuid.as_bytes());
    let _single = shared
        .run_lock
        .try_lock()
        .map_err(|_| "another fixture case is active")?;
    let (identity, signed) = {
        let mut events = shared.events.lock().map_err(|_| "fixture event lock")?;
        let event = events
            .get_mut(&input.event)
            .ok_or("unregistered fixture event")?;
        (event.identity.clone(), event.signed())
    };
    if shared
        .ledger
        .lock()
        .map_err(|_| "fixture ledger lock")?
        .closed
    {
        return Err("fixture ledger closed");
    }
    let started = Instant::now();
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(25))
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "fixture HTTP client")?;
    // An interrupted invocation remains reserved; it cannot silently send again.
    super::config::private_json(
        &shared
            .config
            .private_output
            .join(format!("case-{}.started.json", input.case_id)),
        &json!({"case_sha256":case_sha256,"event_sha256":hash(identity.event_id.as_str().as_bytes()),
            "endpoint_sha256":input.endpoints.iter().map(|endpoint|hash(endpoint.as_bytes())).collect::<Vec<_>>(),
            "request_count":input.endpoints.len()*input.replays_per_ingress,"http_timeout_ms":25000,"terminal_timeout_ms":30000}),
    )?;
    let futures = input
        .endpoints
        .iter()
        .cycle()
        .take(input.endpoints.len() * input.replays_per_ingress)
        .enumerate()
        .map(|(index, endpoint)| {
            super::request::post(
                &client,
                endpoint,
                &shared.channel_id,
                identity.event_id.as_str(),
                &signed,
                index + 1,
                started,
            )
        });
    let requests = futures_util::future::join_all(futures).await;
    // Refused and timed-out HTTP calls still reach the independent terminal oracle.
    let oracle_started = started.elapsed().as_millis();
    let observation = poll::observe(Duration::from_secs(30), || {
        shared
            .oracle
            .observe(&identity, shared.config.replicas_enabled)
    })
    .await;
    let ledger = shared
        .ledger
        .lock()
        .map_err(|_| "fixture ledger lock")?
        .snapshot();
    let result = json!({"schema":"eg_fixture_case_v1","case_sha256":case_sha256,"event_label":input.event,
        "event_sha256":hash(identity.event_id.as_str().as_bytes()),"cohort_clock":"case_local_monotonic","requests":requests,
        "oracle_start_ms":oracle_started,"oracle_end_ms":started.elapsed().as_millis(),"terminal_wait_expired":observation.expired,
        "oracle_observation_count":observation.attempts,
        "oracle_last_successful":observation.last_successful,
        "oracle_last_successful_after_start_ms":observation.last_successful_ms,
        "durable":observation.durable,"ledger":ledger,"status":"OBSERVED_NOT_ACCEPTANCE","http_timeout_ms":25000,"terminal_timeout_ms":30000});
    super::config::private_json(
        &shared
            .config
            .private_output
            .join(format!("case-{}.json", input.case_id)),
        &result,
    )?;
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn invalid_case_or_reused_ingress_is_rejected() {
        let mut run = Run {
            case_id: uuid::Uuid::new_v4().to_string(),
            event: "healthy_a".into(),
            replays_per_ingress: 1,
            endpoints: vec!["http://127.0.0.1:19001".into()],
        };
        assert!(run.validate().is_ok());
        run.endpoints.push(run.endpoints[0].clone());
        assert!(run.validate().is_err());
        run.endpoints[1] = format!("{}/", run.endpoints[0]);
        assert!(run.validate().is_err());
        run.endpoints.pop();
        run.case_id = "untrusted".into();
        assert!(run.validate().is_err());
    }
}
