//! What the bridge must do with a kernel it does not understand.
//!
//! Driven against real processes rather than a stub, because the properties
//! under test are about process boundaries: piped stdio, a stream that ends,
//! a stream that says something on the wrong channel.

use super::*;

/// Wait for a condition, or give up.
///
/// Polls rather than sleeps a fixed time, so these are neither flaky nor
/// slower than the thing they wait for.
async fn until(mut ready: impl AsyncFnMut() -> bool) -> bool {
    for _ in 0..400 {
        if ready().await {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    false
}

#[tokio::test]
async fn frames_cross_in_both_directions_untouched() {
    // `cat` is the honest test double here: it is a real process on real
    // pipes, and it reads exactly as much of a frame as this module does —
    // none of it.
    let bridge = Bridge::start("cat").expect("cat starts");
    let mut heard = bridge.subscribe();

    // Deliberately awkward: an escaped quote, a brace, and a byte outside
    // ASCII. Nothing here reads a frame, so nothing here may be confused by
    // what one contains.
    let sent =
        r#"{"jsonrpc":"2.0","method":"session/prompt","params":{"x":"e\u0301 \" }"}}"#.as_bytes();
    assert!(bridge.send(sent.to_vec()).await);

    let got = tokio::time::timeout(std::time::Duration::from_secs(5), heard.recv())
        .await
        .expect("a frame comes back")
        .expect("the channel is open");
    assert_eq!(
        got,
        Outbound::Frame(sent.to_vec()),
        "a frame must survive the crossing byte for byte"
    );
}

#[tokio::test]
async fn frames_keep_the_order_the_bridge_produced_them_in() {
    let bridge = Bridge::start("cat").expect("cat starts");
    let mut heard = bridge.subscribe();
    for n in 0..20 {
        assert!(bridge.send(format!(r#"{{"seq":{n}}}"#).into_bytes()).await);
    }

    for n in 0..20 {
        let got = tokio::time::timeout(std::time::Duration::from_secs(5), heard.recv())
            .await
            .expect("a frame arrives")
            .expect("the channel is open");
        assert_eq!(
            got,
            Outbound::Frame(format!(r#"{{"seq":{n}}}"#).into_bytes())
        );
    }
}

#[tokio::test]
async fn what_is_said_with_nobody_listening_is_held_for_whoever_comes_back() {
    // The §5 property this exists for. Everything a conversation session did
    // during a control-plane restart would otherwise be gone, where a terminal
    // session simply re-derives its screen.
    let bridge = Bridge::start("cat").expect("cat starts");
    assert!(bridge.send(br#"{"said":"while away"}"#.to_vec()).await);

    let held = until(async || !bridge.pending.lock().await.held.is_empty()).await;
    assert!(held, "a frame with no listener must be held, not dropped");

    let backlog = bridge.backlog().await;
    assert_eq!(
        backlog,
        vec![Outbound::Frame(br#"{"said":"while away"}"#.to_vec())]
    );
    assert!(
        bridge.backlog().await.is_empty(),
        "taking the backlog must empty it, or a reconnect replays twice"
    );
}

#[tokio::test]
async fn a_bridge_that_exits_is_reported_rather_than_going_quiet() {
    // A stream that stops and a runtime that died look identical from the
    // outside, and only one of them is somebody's session ending.
    let bridge = Bridge::start("true").expect("true starts");
    assert!(
        tokio::time::timeout(std::time::Duration::from_secs(5), bridge.ended())
            .await
            .is_ok(),
        "the end of the bridge process must be observable"
    );
}

#[tokio::test]
async fn a_command_that_does_not_exist_fails_at_the_start() {
    // Rather than a bridge that appears to be running and never says anything.
    assert!(Bridge::start("/nonexistent/bridge-binary").is_err());
    assert!(Bridge::start("   ").is_err(), "an empty command is not one");
}

#[test]
fn the_buffer_drops_the_oldest_and_leaves_a_boundary_where_they_were() {
    // A pod cannot grow without limit while its control plane is away, and a
    // conversation that silently skips is worse than one that says where.
    let mut pending = Pending::default();
    let big = vec![b'x'; BUFFER_BYTES / 2];
    for _ in 0..4 {
        pending.push(big.clone());
    }

    let held = pending.drain();
    assert!(
        matches!(held.first(), Some(Outbound::Gap { frames }) if *frames > 0),
        "the gap must be recorded where it happened: {held:?}"
    );
    assert!(
        held.iter()
            .filter(|o| matches!(o, Outbound::Gap { .. }))
            .count()
            == 1,
        "one boundary, not one marker per dropped frame"
    );
    let kept: usize = held
        .iter()
        .filter_map(|o| match o {
            Outbound::Frame(f) => Some(f.len()),
            Outbound::Gap { .. } => None,
        })
        .sum();
    assert!(kept <= BUFFER_BYTES, "the bound must hold: {kept}");
}

#[test]
fn nothing_is_dropped_while_there_is_room() {
    let mut pending = Pending::default();
    for n in 0..50 {
        pending.push(format!(r#"{{"seq":{n}}}"#).into_bytes());
    }
    let held = pending.drain();
    assert_eq!(held.len(), 50);
    assert!(!held.iter().any(|o| matches!(o, Outbound::Gap { .. })));
}

#[test]
fn a_gap_already_recorded_survives_later_dropping() {
    // Losing the record of an earlier gap to make room would leave a
    // conversation that skips with nothing to say it did.
    let mut pending = Pending::default();
    let big = vec![b'x'; BUFFER_BYTES / 2];
    for _ in 0..8 {
        pending.push(big.clone());
    }
    let held = pending.drain();
    assert!(
        held.iter().any(|o| matches!(o, Outbound::Gap { .. })),
        "the boundary must still be there: {held:?}"
    );
}
