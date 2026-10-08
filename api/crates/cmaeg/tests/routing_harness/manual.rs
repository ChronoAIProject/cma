//! Bounded local fixture for external curl and unmodified SDK probes.
use super::{policy::REVIEWER, smoke};
use serde_json::{json, Value};
use std::{path::Path, time::Duration};

fn publish(directory: &Path, name: &str, value: Value) {
    let staging = directory.join(format!("{name}.tmp"));
    std::fs::write(&staging, serde_json::to_vec_pretty(&value).unwrap()).unwrap();
    std::fs::rename(staging, directory.join(name)).unwrap();
}

#[tokio::test]
#[ignore = "serves loopback HTTP until an external curl or SDK probe finishes"]
async fn local_manual_fixture() {
    let directory = std::path::PathBuf::from(std::env::var("CMAEG_MANUAL_DIR").unwrap());
    // A fresh child directory makes stale readiness and finish files impossible.
    std::fs::create_dir(&directory).unwrap();
    let enabled = match std::env::var("CMAEG_MANUAL_ROUTING").unwrap().as_str() {
        "true" => true,
        "false" => false,
        _ => panic!("CMAEG_MANUAL_ROUTING must be true or false"),
    };
    let mut app = smoke::configured(enabled, |raw| {
        raw["routing"]["default_targets"] = json!([REVIEWER]);
        raw["routing"]["rules"] = json!([{"id":"fallback","when":{},"targets":["default"]}]);
        if !enabled {
            raw["reply"]["ack_after_ms"] = 0.into();
            raw["reply"]["auto_final"] = false.into();
        }
    })
    .await;
    publish(
        &directory,
        "config.json",
        json!({"gateway":app.base,"channel":app.channel.id,
        "multi_consumer_enabled":enabled}),
    );
    tokio::time::timeout(Duration::from_secs(120), async {
        while !directory.join("finish").exists() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("external probe must finish before the fixture watchdog");
    app.drain().await;
    let turns = |index| {
        app.turns(index)
            .iter()
            .map(|turn| {
                let context = &turn["event_context"];
                json!({"event_id":context["activity"]["event_id"],
            "routing":context["activity"]["routing"],"input":turn["input"],
            "consumer":turn["metadata"]["cmaeg.consumer"],
            "conversation_id":context["conversation_id"]})
            })
            .collect::<Vec<_>>()
    };
    publish(
        &directory,
        "observations.json",
        json!({"main":turns(0),"reviewer":turns(1),
        "calls":app.peers.calls.0.lock().unwrap().clone()}),
    );
}
