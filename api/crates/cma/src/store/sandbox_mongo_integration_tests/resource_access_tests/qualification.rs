//! Safe evidence from opt-in inventory and observation fixtures.
use serde::Serialize;
use serde_json::{json, Value};
use std::io::Write as _;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

#[derive(Serialize)]
pub(super) struct Sample {
    pub reader: usize,
    pub status: u16,
    pub elapsed_us: u128,
    pub deadline: bool,
    pub semantic_match: bool,
}

pub(super) fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis()
}

pub(super) struct Record {
    data: Value,
    started: Instant,
}

impl Record {
    pub fn new(case: &str, scenario: &str, details: Value) -> Self {
        Self {
            data: json!({
                "schema": "cma-inventory-qualification-v1",
                "case": format!("store::sandbox_mongo_integration_tests::resource_access_tests::{case}"),
                "scenario": scenario, "details": details,
                "debug_assertions": cfg!(debug_assertions),
                "runtime_workers": tokio::runtime::Handle::current().metrics().num_workers(),
                "started_ms": now_ms(), "completed": false,
                "database_cleanup": false, "warmup_completed": 0, "samples": [],
            }),
            started: Instant::now(),
        }
    }

    pub fn set(&mut self, key: &str, value: impl Serialize) {
        self.data[key] = serde_json::to_value(value).unwrap();
    }

    pub fn sample(&mut self, sample: Sample) {
        self.data["samples"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::to_value(sample).unwrap());
    }

    pub fn complete(&mut self, database_cleanup: bool) {
        self.set("database_cleanup", database_cleanup);
        self.set("completed", true);
    }
}

impl Drop for Record {
    fn drop(&mut self) {
        self.data["finished_ms"] = json!(now_ms());
        self.data["elapsed_us"] = json!(self.started.elapsed().as_micros());
        let bytes = format!("CMA_INVENTORY_QUALIFICATION={}\n", self.data);
        let _ = std::io::stdout().lock().write_all(bytes.as_bytes());
    }
}

pub(super) fn availability_selection(
    mode: &str,
    route: &str,
) -> Result<Vec<(bool, bool)>, &'static str> {
    if !matches!(mode, "both" | "legacy" | "batch")
        || !matches!(route, "both" | "agents" | "provenance")
    {
        return Err("invalid availability mode or route");
    }
    Ok([false, true]
        .into_iter()
        .filter(|provenance| route == "both" || *provenance == (route == "provenance"))
        .flat_map(|provenance| {
            [false, true]
                .into_iter()
                .filter(move |batched| mode == "both" || *batched == (mode == "batch"))
                .map(move |batched| (provenance, batched))
        })
        .collect())
}

#[test]
fn availability_selection_is_explicit_and_never_defaults_unknown_values() {
    assert_eq!(
        availability_selection("both", "both").unwrap(),
        vec![(false, false), (false, true), (true, false), (true, true)]
    );
    assert_eq!(
        availability_selection("batch", "agents").unwrap(),
        vec![(false, true)]
    );
    assert_eq!(
        availability_selection("legacy", "provenance").unwrap(),
        vec![(true, false)]
    );
    for (mode, route) in [
        ("", "both"),
        ("batched", "both"),
        ("both", "agent"),
        ("both", ""),
    ] {
        assert!(availability_selection(mode, route).is_err());
    }
}
