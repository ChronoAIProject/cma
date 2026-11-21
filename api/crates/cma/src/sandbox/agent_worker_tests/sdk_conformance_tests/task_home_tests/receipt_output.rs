use serde_json::{json, Value};

const MARKER: &str = "TASK_HOME_RECEIPT=";

/// Serial libtest can put the receipt after the test name on its progress line.
pub(super) fn parse_task_home_receipt(output: &str) -> Value {
    let (_, payload) = output
        .split_once(MARKER)
        .unwrap_or_else(|| panic!("child output did not contain {MARKER}"));
    let mut values = serde_json::Deserializer::from_str(payload).into_iter::<Value>();
    let receipt = values
        .next()
        .unwrap_or_else(|| panic!("{MARKER} marker had no JSON payload"))
        .unwrap_or_else(|error| panic!("{MARKER} payload was invalid JSON: {error}"));
    assert!(
        receipt.is_object(),
        "{MARKER} payload must be a JSON object"
    );
    // A string inside the decoded receipt can contain the same literal marker.
    assert!(
        !payload[values.byte_offset()..].contains(MARKER),
        "child output contained more than one {MARKER} marker"
    );
    receipt
}

#[test]
fn accepts_standalone_receipt_line() {
    let receipt = parse_task_home_receipt(
        "TASK_HOME_RECEIPT={\"receipt\":{\"id\":\"op\"},\"agent_id\":\"agt\"}\n",
    );
    assert_eq!(receipt, json!({"receipt": {"id": "op"}, "agent_id": "agt"}));
}

#[test]
fn accepts_progress_prefix_and_trailing_framing() {
    let receipt = parse_task_home_receipt(
        "test task_home_create_child ... TASK_HOME_RECEIPT={\"agent_id\":\"agt\"}ok\ntest result: ok. 1 passed\n",
    );
    assert_eq!(receipt["agent_id"], "agt");
}

#[test]
fn accepts_marker_like_text_inside_receipt() {
    let receipt = parse_task_home_receipt(
        "TASK_HOME_RECEIPT={\"receipt\":{\"title\":\"Text with TASK_HOME_RECEIPT= inside\"}}\nok\n",
    );
    assert_eq!(
        receipt["receipt"]["title"],
        "Text with TASK_HOME_RECEIPT= inside"
    );
}

#[test]
#[should_panic(expected = "more than one TASK_HOME_RECEIPT=")]
fn rejects_duplicate_markers_outside_receipt() {
    parse_task_home_receipt("TASK_HOME_RECEIPT={\"agent_id\":\"one\"}\nTASK_HOME_RECEIPT={}");
}

#[test]
#[should_panic(expected = "did not contain TASK_HOME_RECEIPT=")]
fn rejects_missing_marker() {
    parse_task_home_receipt("test result: ok. 1 passed");
}

#[test]
#[should_panic(expected = "marker had no JSON payload")]
fn rejects_empty_payload() {
    parse_task_home_receipt("TASK_HOME_RECEIPT=\n");
}

#[test]
#[should_panic(expected = "payload was invalid JSON")]
fn rejects_malformed_payload() {
    parse_task_home_receipt("TASK_HOME_RECEIPT={\"agent_id\":\n");
}

#[test]
#[should_panic(expected = "payload must be a JSON object")]
fn rejects_nonobject_payload() {
    parse_task_home_receipt("TASK_HOME_RECEIPT=[\"agt\"]\n");
}

#[test]
#[ignore = "invoked only by parses_serial_libtest_child_output"]
fn matching_child() {
    println!("{MARKER}{}", json!({"agent_id": "synthetic-agent"}));
}

#[test]
fn parses_serial_libtest_child_output() {
    let (_, module) = module_path!().split_once("::").unwrap();
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            &format!("{module}::matching_child"),
            "--ignored",
            "--nocapture",
        ])
        .env("RUST_TEST_THREADS", "1")
        .output()
        .unwrap();
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(output.status.success(), "{stdout}");
    assert!(stdout.contains("1 passed"), "{stdout}");
    assert_eq!(
        parse_task_home_receipt(&stdout)["agent_id"],
        "synthetic-agent"
    );
}
