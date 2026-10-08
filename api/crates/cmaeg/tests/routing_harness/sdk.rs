//! The existing SDK streaming clients against a real routed gateway composition.
use super::{policy::REVIEWER, smoke};
use serde_json::json;
use std::{path::PathBuf, time::Duration};
use tokio::process::Command;

#[tokio::test]
#[ignore = "requires built TypeScript SDK and CMAEG_ROUTED_SDK_RUST_BIN"]
async fn both_unmodified_sdk_streams_accept_a_single_routed_consumer() {
    let rust_binary = std::env::var_os("CMAEG_ROUTED_SDK_RUST_BIN")
        .expect("prebuild sdk/rust cmaeg_routed_fixture to avoid nested Cargo locks");
    let mut app = smoke::app(|_| {}).await;
    let directory = std::env::temp_dir().join(format!("cmaeg-sdk-routed-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    let config = directory.join("config.json");
    std::fs::write(
        &config,
        json!({"gateway":app.base,"channel":app.channel.id}).to_string(),
    )
    .unwrap();
    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let mut typescript = Command::new("node");
    typescript
        .arg("scripts/cmaeg-routed-fixture.mjs")
        .current_dir(repo.join("sdk/typescript"));
    let mut rust = Command::new(rust_binary);
    rust.args([
        "--ignored",
        "--nocapture",
        "--exact",
        "plain_stream_accepts_one_routed_consumer",
    ]);
    for command in [&mut typescript, &mut rust] {
        let output = tokio::time::timeout(
            Duration::from_secs(30),
            command
                .kill_on_drop(true)
                .env("CMAEG_MANUAL_CONFIG", &config)
                .output(),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(
            output.status.success(),
            "SDK probe failed: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        print!("{}", String::from_utf8(output.stdout).unwrap());
    }
    app.drain().await;
    assert!(app.turns(0).is_empty());
    let turns = app.turns(1);
    assert_eq!(turns.len(), 2);
    for (turn, id) in turns.iter().zip(["ts-routed-probe", "rust-routed-probe"]) {
        assert_eq!(turn["event_context"]["activity"]["event_id"], id);
        assert_eq!(
            turn["event_context"]["activity"]["routing"]["targets"],
            json!([{"id":REVIEWER,"name":"reviewer"}])
        );
        assert_eq!(turn["metadata"]["cmaeg.consumer"], REVIEWER);
    }
    assert!(app.peers.chat_calls().is_empty());
    std::fs::remove_dir_all(directory).unwrap();
}
