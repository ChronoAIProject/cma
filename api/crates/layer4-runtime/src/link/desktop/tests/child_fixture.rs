//! Explicit subprocess-only fixture for public CMA/adapter integration tests.
use super::test_support::Fixture;
use std::io::{BufRead, Read, Write};

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Settings {
    sandbox_id: String,
    root: std::path::PathBuf,
    control_token: String,
}

#[test]
#[ignore = "child helper; requires explicit stdin configuration and parent cleanup"]
fn desktop_receiver_child() {
    assert_eq!(
        std::env::var("CMA_DESKTOP_RECEIVER_CHILD").as_deref(),
        Ok("1")
    );
    let mut input = std::io::stdin().lock();
    let mut line = String::new();
    input
        .by_ref()
        .take(8193)
        .read_line(&mut line)
        .expect("fixture settings input");
    assert!(
        line.len() <= 8192 && line.ends_with('\n'),
        "invalid fixture settings length"
    );
    let settings: Settings =
        serde_json::from_str(&line).unwrap_or_else(|_| panic!("invalid fixture settings"));
    assert!(
        !settings.sandbox_id.is_empty()
            && settings.sandbox_id.len() <= 256
            && !settings.sandbox_id.chars().any(char::is_control)
    );
    assert!(
        settings.root.is_absolute() && settings.root.symlink_metadata().is_ok_and(|m| m.is_dir()),
        "parent must supply a dedicated existing root directory"
    );
    assert!(
        settings.control_token.len() == 64
            && settings
                .control_token
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "invalid fixture credential format"
    );
    let mut config = crate::config::RuntimeConfig::for_test(settings.root.join("home"));
    config.sandbox_id = settings.sandbox_id.clone();
    config.state_root = settings.root.join("state");
    config.checkpoint_root = settings.root.join("checkpoints");
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("fixture runtime");
    let fixture = runtime.block_on(Fixture::start(config, settings.control_token));
    println!(
        "CMA_DESKTOP_RECEIVER_READY {}",
        serde_json::json!({
            "protocol": 1, "address": fixture.address, "sandbox_id": settings.sandbox_id,
            "generation": 1, "desktop_path": "/runtime/desktop",
            "control_path": "/runtime/desktop/ownership",
            "floor_path": "state/desktop-owner/floor.json",
            "activation_path": "state/desktop-owner.activated"
        })
    );
    std::io::stdout().flush().expect("fixture readiness flush");
    let mut extra = [0];
    assert_eq!(
        input.read(&mut extra).expect("fixture shutdown input"),
        0,
        "only EOF is permitted after settings"
    );
    runtime.block_on(fixture.stop());
}
