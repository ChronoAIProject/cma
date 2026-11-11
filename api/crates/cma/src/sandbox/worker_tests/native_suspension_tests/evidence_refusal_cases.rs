use serde_json::Value;
use std::{
    io::Write,
    process::{Command, Stdio},
};

pub(super) fn provider_refusal_cases() -> Vec<(&'static str, &'static str, Option<Value>)> {
    let mut child = Command::new("python3")
        .args([
            "-I",
            "-c",
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../layer3-adapter/src/codex/recovery_refusal_fixture.py"
            )),
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(
            concat!(
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../layer3-adapter/src/codex/recovery_io.py"
                )),
                "\n",
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/../layer3-adapter/src/codex/recovery.py"
                )),
            )
            .as_bytes(),
        )
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut responses: serde_json::Map<String, Value> =
        serde_json::from_slice(&output.stdout).unwrap();
    let mut cases = Vec::new();
    for (case, reason) in [
        ("missing_original", "CorruptHistory"),
        ("metadata_absent", "CorruptHistory"),
        ("metadata_malformed", "CorruptHistory"),
        ("metadata_duplicate", "CorruptHistory"),
        ("conversation_duplicate", "CorruptHistory"),
        ("history_symlink", "CorruptHistory"),
        ("history_path", "CorruptHistory"),
        ("history_empty", "CorruptHistory"),
        ("history_file_limit", "CorruptHistory"),
        ("history_incomplete", "CorruptHistory"),
        ("history_invalid", "CorruptHistory"),
        ("evidence_io", "Unavailable"),
        ("evidence_decode", "Unavailable"),
        ("evidence_type", "Unavailable"),
        ("history_missing", "MissingHistory"),
        ("history_replaced", "CorruptHistory"),
        ("history_prefix_changed", "CorruptHistory"),
        ("history_descriptor", "CorruptHistory"),
        ("turn_identity", "CorruptHistory"),
        ("turn_limit", "CorruptHistory"),
        ("native_turn_unsettled", "CorruptHistory"),
        ("native_turn_restarted", "CorruptHistory"),
        ("request_invalid", "Ineligible"),
    ] {
        cases.push((case, reason, Some(responses.remove(case).unwrap())));
    }
    assert!(
        responses.is_empty(),
        "every emitted provider fixture must reach the verifier"
    );
    // Unknown response values never become labels or trigger substring/error
    // parsing. The same explicit fallback also covers a malformed code field.
    for (case, code) in [
        ("unknown_code", serde_json::json!("unrecognized_refusal")),
        (
            "unknown_error_text",
            serde_json::json!("evidence_unavailable: private provider detail"),
        ),
        ("unknown_code_type", serde_json::json!(42)),
    ] {
        cases.push((
            case,
            "Ineligible",
            Some(serde_json::json!({
                "version": 1, "status": "refused", "code": code
            })),
        ));
    }
    cases
}
