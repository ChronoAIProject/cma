//! What a link says about itself, and how a frame is read on it.
//!
//! The wire is the contract between a pod and a control plane that may have
//! been deployed at a different time. Everything here is about the two of them
//! agreeing without having to guess.

use super::*;
use crate::bridge::Outbound;
use crate::conversation_link::{gap, outbound};
use crate::lanes::{LANE_CONVERSATION, LANE_CONVERSATION_GAP};
use std::path::PathBuf;
use std::time::Duration;

struct ResourceFixture {
    root: PathBuf,
}

impl ResourceFixture {
    fn new(name: &str, observed: bool) -> Self {
        let root = std::env::temp_dir().join(format!(
            "chrono-link-resources-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("proc/self")).expect("proc fixture is created");
        std::fs::create_dir_all(root.join("cgroup")).expect("cgroup fixture is created");
        std::fs::write(root.join("proc/self/cgroup"), "0::/\n")
            .expect("membership fixture is written");
        if observed {
            std::fs::write(root.join("cgroup/memory.current"), "1\n")
                .expect("resource fixture is written");
        }
        Self { root }
    }

    fn roots(&self) -> crate::resources::Roots {
        crate::resources::Roots {
            proc: self.root.join("proc"),
            cgroup_mount: self.root.join("cgroup"),
            net: self.root.join("net"),
        }
    }
}

impl Drop for ResourceFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn resource_state(name: &str, observed: bool) -> (AppState, ResourceFixture) {
    let fixture = ResourceFixture::new(name, observed);
    let mut state = AppState::for_test(&[]);
    state.resources = std::sync::Arc::new(crate::resources::Sampler::new(
        fixture.roots(),
        std::time::Instant::now(),
    ));
    (state, fixture)
}

/// A link opened with these query values, as the control plane would.
fn params(wire: Option<u32>, lanes: Option<&str>) -> AttachParams {
    AttachParams {
        cols: Some(120),
        rows: Some(40),
        wire,
        lanes: lanes.map(str::to_string),
        telemetry: None,
    }
}

#[test]
fn a_control_plane_that_says_nothing_gets_the_wire_it_had() {
    // The §5 half that is easy to forget: a control plane deployed before
    // lanes existed opens links with no `wire`, and must keep getting
    // unprefixed binary. A pod that framed anyway would write a discriminator
    // into somebody's terminal as a typed character.
    let p = params(None, None);
    assert!(!p.framed());
    assert!(!p.conversational());
}

#[test]
fn asking_for_the_lane_wire_is_what_turns_framing_on() {
    let p = params(Some(1), None);
    assert!(p.framed());
    assert!(
        !p.conversational(),
        "the terminal is still the default lane"
    );
}

#[test]
fn a_wire_older_than_this_pod_speaks_is_treated_as_the_legacy_one() {
    // Conservative on purpose: the only safe reading of a version this pod
    // does not implement is the framing that predates versions.
    assert!(!params(Some(0), None).framed());
}

#[test]
fn the_conversation_lane_is_asked_for_by_name() {
    let p = params(Some(1), Some("conv"));
    assert!(p.framed());
    assert!(p.conversational());
}

#[test]
fn an_unfamiliar_lane_name_is_not_the_conversation() {
    // Nothing is inferred from a name this pod does not serve; it drives the
    // terminal, which is what a link has always been for.
    assert!(!params(Some(1), Some("something-else")).conversational());
}

#[test]
fn a_gap_says_how_many_frames_fell_in_it() {
    // The control plane records a boundary its owner can be shown, so the
    // count has to survive the crossing.
    let Message::Binary(bytes) = gap(17) else {
        panic!("a gap travels as binary, on its own lane");
    };
    let (lane, payload) = lanes::unframe(&bytes).expect("carries a lane");
    assert_eq!(lane, LANE_CONVERSATION_GAP);
    let parsed: serde_json::Value = serde_json::from_slice(payload).expect("the marker is json");
    assert_eq!(parsed["frames"], 17);
}

#[test]
fn a_conversation_frame_crosses_on_its_own_lane_untouched() {
    let said = br#"{"jsonrpc":"2.0","method":"session/update"}"#;
    let Message::Binary(bytes) = outbound(Outbound::Frame(said.to_vec())) else {
        panic!("a conversation frame travels as binary");
    };
    let (lane, payload) = lanes::unframe(&bytes).expect("carries a lane");
    assert_eq!(lane, LANE_CONVERSATION);
    assert_eq!(payload, said, "a frame must not be reshaped in transit");
}

#[test]
fn the_control_vocabulary_still_parses_from_its_tag() {
    // Unchanged from the single-lane wire, which is what lets a pod that
    // predates lanes and one that does not accept the same control frames.
    let credential: ControlFrame =
        serde_json::from_str(r#"{"t":"credential","value":"x"}"#).expect("credential parses");
    assert!(matches!(credential, ControlFrame::Credential { .. }));
    let resize: ControlFrame =
        serde_json::from_str(r#"{"t":"resize","cols":80,"rows":24}"#).expect("resize parses");
    assert!(matches!(
        resize,
        ControlFrame::Resize { cols: 80, rows: 24 }
    ));
    let signal: ControlFrame =
        serde_json::from_str(r#"{"t":"signal","sig":"INT"}"#).expect("signal parses");
    assert!(matches!(signal, ControlFrame::Signal { .. }));
    assert!(serde_json::from_str::<ControlFrame>(r#"{"t":"nope"}"#).is_err());
}

#[test]
fn a_switch_names_the_program_to_restart() {
    // The command is the kernel's own front-end name, carried verbatim; the
    // allowlist check happens when it is acted on, not when it is parsed.
    let switch: ControlFrame =
        serde_json::from_str(r#"{"t":"switch_plane","command":"codex-acp"}"#)
            .expect("a switch parses");
    assert!(matches!(
        switch,
        ControlFrame::SwitchPlane { command } if command == "codex-acp"
    ));
}

async fn captured_signal_input(signals: &[&str]) -> Vec<u8> {
    let state = AppState::for_test(&["/bin/echo"]);
    let (session, _output) = state
        .registry
        .get_or_start("signal-map", "/bin/echo", 80, 24, "k")
        .await
        .expect("the test terminal starts");
    let captured = session.capture_input_writes_for_test();
    let (replies, _answers) = mpsc::channel::<String>(CONTROL_REPLIES);

    for signal in signals {
        let frame = serde_json::json!({ "t": "signal", "sig": signal }).to_string();
        control(&frame, "signal-map", &state, Some(&session), &replies).await;
    }

    let bytes = captured
        .lock()
        .expect("the captured input is healthy")
        .clone();
    bytes
}

#[tokio::test]
async fn every_documented_signal_writes_its_exact_control_byte() {
    assert_eq!(
        captured_signal_input(&["INT", "TERM", "QUIT", "HUP"]).await,
        vec![0x03, 0x04, 0x1c, 0x04]
    );
}

#[tokio::test]
async fn an_unknown_signal_writes_nothing() {
    assert!(captured_signal_input(&["XYZ"]).await.is_empty());
}

#[tokio::test]
async fn a_switch_is_answered_either_way() {
    // #289: the frame reaching the socket says nothing about the program
    // being swapped, so the supervisor answers — and the control plane
    // records an in-place switch only on a positive answer.
    let state = AppState::for_test(&["codex-acp", "codex"]);
    let (replies, mut answers) = mpsc::channel::<String>(CONTROL_REPLIES);

    control(
        r#"{"t":"switch_plane","command":"codex-acp"}"#,
        "sbx_1",
        &state,
        None,
        &replies,
    )
    .await;
    // Asserted on the serialised frame rather than on a Rust value: the two
    // ends agree on what goes over the wire. Key order is not part of that
    // agreement — a workspace build turns on serde_json's preserve_order and
    // a crate build does not — so the frame is parsed and its fields read.
    let accepted: serde_json::Value =
        serde_json::from_str(&answers.recv().await.expect("the switch was answered"))
            .expect("the answer is JSON");
    assert_eq!(accepted["t"], "switch_plane_result");
    assert_eq!(accepted["command"], "codex-acp");
    assert_eq!(accepted["accepted"], true);

    // A command this pod was never told it may run is refused by name, and
    // the refusal is on the wire rather than only in a log line.
    control(
        r#"{"t":"switch_plane","command":"rm"}"#,
        "sbx_1",
        &state,
        None,
        &replies,
    )
    .await;
    let refused: serde_json::Value =
        serde_json::from_str(&answers.recv().await.expect("the refusal was answered"))
            .expect("the refusal is JSON");
    assert_eq!(refused["t"], "switch_plane_result");
    assert_eq!(refused["command"], "rm");
    assert_eq!(refused["accepted"], false);
}

type ClientSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn serve(state: AppState) -> (std::net::SocketAddr, tokio::task::JoinHandle<()>) {
    let app = axum::Router::new()
        .route("/readyz", axum::routing::get(crate::ready))
        .route("/runtime/{id}/terminal", axum::routing::get(attach))
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("the test listener binds");
    let address = listener.local_addr().expect("the listener has an address");
    let server = tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("the test runtime serves");
    });
    (address, server)
}

async fn telemetry_link(address: std::net::SocketAddr, interval_ms: u64) -> ClientSocket {
    let url =
        format!("ws://{address}/runtime/sbx_telemetry/terminal?wire=1&telemetry={interval_ms}");
    tokio_tungstenite::connect_async(url)
        .await
        .expect("the telemetry link opens")
        .0
}

async fn terminal_link(address: std::net::SocketAddr, id: &str) -> ClientSocket {
    let url = format!("ws://{address}/runtime/{id}/terminal?wire=1&cols=120&rows=40");
    tokio_tungstenite::connect_async(url)
        .await
        .expect("the terminal link opens")
        .0
}

async fn next_client_message(socket: &mut ClientSocket) -> tokio_tungstenite::tungstenite::Message {
    tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("the runtime link answers in time")
        .expect("the runtime link remains open")
        .expect("the runtime frame is readable")
}

async fn terminal_output_until_exit(socket: &mut ClientSocket) -> (Vec<u8>, serde_json::Value) {
    let mut output = Vec::new();
    loop {
        match next_client_message(socket).await {
            tokio_tungstenite::tungstenite::Message::Binary(bytes) => {
                let (lane, payload) = lanes::unframe(&bytes).expect("terminal output has a lane");
                assert_eq!(lane, LANE_TERMINAL);
                output.extend_from_slice(payload);
            }
            tokio_tungstenite::tungstenite::Message::Text(text) => {
                let ending: serde_json::Value =
                    serde_json::from_str(&text).expect("the process ending is JSON");
                assert_eq!(ending["t"], "exit");
                return (output, ending);
            }
            other => panic!("the process ended before reporting its status: {other:?}"),
        }
    }
}

async fn assert_normal_close(socket: &mut ClientSocket) {
    let tokio_tungstenite::tungstenite::Message::Close(Some(close)) =
        next_client_message(socket).await
    else {
        panic!("the exit report must be followed by a close frame");
    };
    assert_eq!(
        close.code,
        tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Normal
    );
}

#[tokio::test]
async fn a_pty_write_failure_waits_for_the_output_pump_to_finish() {
    let (release_output, output_released) = tokio::sync::oneshot::channel();
    let (output_finished, finished_output) = tokio::sync::oneshot::channel();
    let (input_finished, finished_input) = tokio::sync::oneshot::channel();

    let pump_out = tokio::spawn(async move {
        let _ = output_released.await;
        let _ = output_finished.send(());
    });
    let pump_in = tokio::spawn(async move {
        let _ = input_finished.send(());
        TerminalInputEnd::PtyWriteFailed
    });
    let finishing = tokio::spawn(finish_terminal_pumps(pump_out, pump_in));

    finished_input
        .await
        .expect("the failed input pump completes first");
    release_output
        .send(())
        .expect("the output pump is still being awaited");
    tokio::time::timeout(Duration::from_secs(1), finishing)
        .await
        .expect("pump arbitration finishes")
        .expect("pump arbitration does not panic");
    finished_output
        .await
        .expect("a PTY write failure must not abort the ending reporter");
}

fn assert_telemetry_frame(message: tokio_tungstenite::tungstenite::Message) {
    let tokio_tungstenite::tungstenite::Message::Binary(bytes) = message else {
        panic!("a resource sample travels as binary");
    };
    let (lane, payload) = lanes::unframe(&bytes).expect("the sample carries a lane");
    assert_eq!(lane, lanes::LANE_TELEMETRY);
    let frame: serde_json::Value = serde_json::from_slice(payload).expect("the sample is JSON");
    assert!(frame["uptimeMs"].is_number());
}

#[tokio::test]
async fn process_output_precedes_its_success_status_and_normal_close() {
    let state = AppState::for_test(&["/bin/echo final-output"]);
    let (address, server) = serve(state).await;
    let mut socket = terminal_link(address, "sbx_success").await;

    let tokio_tungstenite::tungstenite::Message::Text(ready) =
        next_client_message(&mut socket).await
    else {
        panic!("the lane declaration must be the first frame");
    };
    let ready: serde_json::Value = serde_json::from_str(&ready).expect("the declaration is JSON");
    assert_eq!(ready["t"], "ready");

    let (output, exit) = terminal_output_until_exit(&mut socket).await;
    assert!(
        String::from_utf8_lossy(&output).contains("final-output"),
        "the process's final output must arrive before its exit report"
    );
    assert_eq!(exit["code"], 0);
    assert_normal_close(&mut socket).await;
    server.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn input_after_process_exit_cannot_cancel_its_reported_ending() {
    let state = AppState::for_test(&["/bin/sh"]);
    let inspected = state.clone();
    let (address, server) = serve(state).await;
    let mut socket = terminal_link(address, "sbx_late_input").await;
    assert!(matches!(
        next_client_message(&mut socket).await,
        tokio_tungstenite::tungstenite::Message::Text(_)
    ));

    let session = inspected
        .registry
        .session_for_test("sbx_late_input")
        .await
        .expect("the attached process is registered");
    let mut exited = session.subscribe_exit();
    socket
        .send(tokio_tungstenite::tungstenite::Message::Binary(
            lanes::frame(
                LANE_TERMINAL,
                b"head -c 16777216 /dev/zero; printf late-; printf final-output; exit 31\n",
            ),
        ))
        .await
        .expect("the process receives its ending command");

    // Do not read the large output yet. This holds the socket's output pump in
    // a send while the supervisor independently reaps and latches the child.
    tokio::time::timeout(Duration::from_secs(10), exited.changed())
        .await
        .expect("the process exits while its socket output is backed up")
        .expect("the exit watch remains open");
    assert_eq!(exited.borrow().expect("an exit was latched").code, Some(31));
    // Some kernels accept writes to a retained PTY master after the slave has
    // gone while others return EIO. Inject the same failure on every platform
    // so this exercises the input pump's failure branch deterministically.
    let input_failed = session.fail_input_writes_for_test();

    socket
        .send(tokio_tungstenite::tungstenite::Message::Binary(
            lanes::frame(LANE_TERMINAL, b"late"),
        ))
        .await
        .expect("the in-flight keystroke reaches the runtime link");
    tokio::time::timeout(Duration::from_secs(1), input_failed)
        .await
        .expect("the runtime reads the late input in time")
        .expect("the injected writer observes the late input");

    let (output, exit) = terminal_output_until_exit(&mut socket).await;
    assert!(
        String::from_utf8_lossy(&output).contains("late-final-output"),
        "final output must still precede the ending"
    );
    assert_eq!(exit["code"], 31);
    assert_normal_close(&mut socket).await;
    server.abort();
}

#[tokio::test]
async fn signal_termination_reports_no_numeric_status_before_normal_close() {
    let state = AppState::for_test(&["/bin/sh"]);
    let (address, server) = serve(state).await;
    let mut socket = terminal_link(address, "sbx_signal").await;
    assert!(matches!(
        next_client_message(&mut socket).await,
        tokio_tungstenite::tungstenite::Message::Text(_)
    ));

    socket
        .send(tokio_tungstenite::tungstenite::Message::Binary(
            lanes::frame(LANE_TERMINAL, b"exec /bin/sh -c 'kill -TERM $$'\n"),
        ))
        .await
        .expect("the signal command reaches the shell");

    let (_output, exit) = terminal_output_until_exit(&mut socket).await;
    assert_eq!(exit["code"], serde_json::Value::Null);
    assert_normal_close(&mut socket).await;
    server.abort();
}

#[tokio::test]
async fn saturated_terminal_output_does_not_starve_a_control_reply() {
    let state = AppState::for_test(&["/bin/sh", "codex-acp"]);
    let registry = state.registry.clone();
    let (address, server) = serve(state).await;
    let mut socket = terminal_link(address, "sbx_fair_control").await;
    assert!(matches!(
        next_client_message(&mut socket).await,
        tokio_tungstenite::tungstenite::Message::Text(_)
    ));

    let session = registry
        .session_for_test("sbx_fair_control")
        .await
        .expect("the test terminal is registered");
    let flooding = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
    let producer = tokio::spawn({
        let flooding = std::sync::Arc::clone(&flooding);
        async move {
            while flooding.load(std::sync::atomic::Ordering::Acquire) {
                // Refill beyond the lossy ring's capacity before yielding, so
                // the output receive remains ready at every select point.
                for _ in 0..1_024 {
                    session.publish_output_for_test(b"saturated-output\n");
                }
                tokio::task::yield_now().await;
            }
        }
    });
    loop {
        if let tokio_tungstenite::tungstenite::Message::Binary(bytes) =
            next_client_message(&mut socket).await
        {
            if lanes::unframe(&bytes).is_some_and(|(lane, payload)| {
                lane == LANE_TERMINAL && payload == b"saturated-output\n"
            }) {
                break;
            }
        }
    }

    socket
        .send(tokio_tungstenite::tungstenite::Message::Text(
            serde_json::json!({
                "t": "switch_plane",
                "command": "codex-acp",
            })
            .to_string(),
        ))
        .await
        .expect("the switch request reaches the supervisor");

    let reply = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let tokio_tungstenite::tungstenite::Message::Text(text) =
                next_client_message(&mut socket).await
            {
                let value: serde_json::Value =
                    serde_json::from_str(&text).expect("the control reply is JSON");
                if value["t"] == SWITCH_RESULT {
                    break value;
                }
            }
        }
    })
    .await;
    let reply = match reply {
        Ok(reply) => reply,
        Err(_) => {
            flooding.store(false, std::sync::atomic::Ordering::Release);
            producer.abort();
            server.abort();
            panic!("continuous terminal output starved control traffic");
        }
    };
    assert_eq!(reply["accepted"], true);

    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if let tokio_tungstenite::tungstenite::Message::Binary(bytes) =
                next_client_message(&mut socket).await
            {
                if lanes::unframe(&bytes).is_some_and(|(lane, payload)| {
                    lane == LANE_TERMINAL && payload == b"saturated-output\n"
                }) {
                    break;
                }
            }
        }
    })
    .await
    .expect("terminal output remains pending after the control reply");
    flooding.store(false, std::sync::atomic::Ordering::Release);
    tokio::time::timeout(Duration::from_secs(1), producer)
        .await
        .expect("the bounded output producer stops")
        .expect("the bounded output producer remains healthy");

    socket
        .send(tokio_tungstenite::tungstenite::Message::Binary(
            lanes::frame(LANE_TERMINAL, b"exit\n"),
        ))
        .await
        .expect("the test shell is ended");
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let tokio_tungstenite::tungstenite::Message::Text(text) =
                next_client_message(&mut socket).await
            {
                let value: serde_json::Value =
                    serde_json::from_str(&text).expect("the process ending is JSON");
                if value["t"] == "exit" {
                    break;
                }
            }
        }
    })
    .await
    .expect("the test shell exits after its test output is drained");
    server.abort();
}

#[tokio::test]
async fn a_link_that_names_a_cadence_is_a_telemetry_link_and_takes_no_pty() {
    let (state, _resources) = resource_state("telemetry-link", true);
    let inspected = state.clone();
    let (address, server) = serve(state).await;
    let mut socket = telemetry_link(address, 2000).await;

    let tokio_tungstenite::tungstenite::Message::Text(ready) =
        next_client_message(&mut socket).await
    else {
        panic!("the lane declaration travels first as text");
    };
    let ready: serde_json::Value = serde_json::from_str(&ready).expect("the declaration is JSON");
    assert_eq!(ready["lanes"], serde_json::json!(["ctl", "tel"]));
    assert_telemetry_frame(next_client_message(&mut socket).await);
    assert_telemetry_frame(next_client_message(&mut socket).await);
    assert_eq!(inspected.registry.len().await, 0);
    server.abort();
}

#[tokio::test]
async fn a_cadence_outside_what_this_pod_serves_is_clamped_and_not_refused() {
    assert_eq!(
        crate::telemetry::clamp_interval_ms(1),
        crate::telemetry::MIN_INTERVAL_MS
    );
    assert_eq!(
        crate::telemetry::clamp_interval_ms(3_600_000),
        crate::telemetry::MAX_INTERVAL_MS
    );
    let (state, _resources) = resource_state("clamped-telemetry", true);
    let (address, server) = serve(state).await;
    for requested in [1, 3_600_000] {
        let mut socket = telemetry_link(address, requested).await;
        assert!(matches!(
            next_client_message(&mut socket).await,
            tokio_tungstenite::tungstenite::Message::Text(_)
        ));
        assert_telemetry_frame(next_client_message(&mut socket).await);
    }
    server.abort();
}

#[tokio::test]
async fn readiness_declares_the_lane_this_image_serves() {
    let (state, _resources) = resource_state("readiness-observed", true);
    let (address, server) = serve(state).await;
    let ready: serde_json::Value = reqwest::get(format!("http://{address}/readyz"))
        .await
        .expect("readiness is reachable")
        .json()
        .await
        .expect("readiness is JSON");
    assert_eq!(ready["prelude"], true);
    assert_eq!(ready["resources"], true);
    server.abort();
}

#[tokio::test]
async fn readiness_does_not_advertise_an_uptime_only_sampler() {
    let (state, _resources) = resource_state("readiness-empty", false);
    let (address, server) = serve(state).await;
    let ready: serde_json::Value = reqwest::get(format!("http://{address}/readyz"))
        .await
        .expect("readiness is reachable")
        .json()
        .await
        .expect("readiness is JSON");
    assert_eq!(ready["resources"], false);
    server.abort();
}

#[tokio::test]
async fn an_uptime_only_sample_is_not_sent_as_telemetry() {
    let (state, _resources) = resource_state("empty-telemetry", false);
    let (address, server) = serve(state).await;
    let mut socket = telemetry_link(address, crate::telemetry::MIN_INTERVAL_MS).await;
    assert!(matches!(
        next_client_message(&mut socket).await,
        tokio_tungstenite::tungstenite::Message::Text(_)
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(400), socket.next())
            .await
            .is_err(),
        "no uptime-only telemetry frame should arrive"
    );
    server.abort();
}
