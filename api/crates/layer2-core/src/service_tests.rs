//! Tests for the session service's terminal relay.
//!
//! Kept in a sibling file so `service.rs` stays within the project's file-size
//! limit while the behaviour remains covered.

use super::*;
use crate::terminal::{ScreenSnapshot, TerminalGeometry};

/// A screen model that records what it was fed, so the relay's behaviour
/// can be observed without a real terminal parser.
struct RecordingScreen {
    geometry: TerminalGeometry,
    seen: Vec<u8>,
}

impl ScreenModel for RecordingScreen {
    fn apply(&mut self, bytes: &[u8]) {
        self.seen.extend_from_slice(bytes);
    }

    fn resize(&mut self, geometry: TerminalGeometry) {
        self.geometry = geometry;
    }

    fn snapshot(&self) -> ScreenSnapshot {
        ScreenSnapshot {
            geometry: self.geometry,
            rows: vec![String::from_utf8_lossy(&self.seen).into_owned()],
        }
    }

    fn repaint_bytes(&self) -> Vec<u8> {
        self.seen.clone()
    }
}

#[tokio::test]
async fn the_relay_feeds_the_screen_with_no_client_attached() {
    // The whole point of the relay: a detached session keeps working, and
    // its screen stays current so a later attach is correct.
    let (tx, rx) = mpsc::channel::<Vec<u8>>(8);
    let geometry = TerminalGeometry::default();
    let screen: Arc<Mutex<Box<dyn ScreenModel>>> =
        Arc::new(Mutex::new(Box::new(RecordingScreen {
            geometry,
            seen: Vec::new(),
        })));
    let (live, _) = broadcast::channel(8);

    spawn_relay(SessionId("sesn_test".into()), rx, Arc::clone(&screen), live);

    tx.send(b"painted".to_vec())
        .await
        .expect("relay accepts output");
    // Yield until the relay has drained the channel.
    for _ in 0..50 {
        if !screen.lock().await.repaint_bytes().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(
        screen.lock().await.repaint_bytes(),
        b"painted".to_vec(),
        "the screen must absorb output even with nobody attached"
    );
}

#[tokio::test]
async fn output_with_no_subscriber_is_not_an_error() {
    // A send into a broadcast with no receivers fails; the relay must treat
    // that as "detached", not as a fault that ends the session.
    let (tx, rx) = mpsc::channel::<Vec<u8>>(8);
    let screen: Arc<Mutex<Box<dyn ScreenModel>>> =
        Arc::new(Mutex::new(Box::new(RecordingScreen {
            geometry: TerminalGeometry::default(),
            seen: Vec::new(),
        })));
    let (live, _) = broadcast::channel(8);
    spawn_relay(SessionId("sesn_test".into()), rx, Arc::clone(&screen), live);

    for chunk in [b"one".to_vec(), b"two".to_vec()] {
        tx.send(chunk).await.expect("relay still accepting");
    }
    for _ in 0..50 {
        if screen.lock().await.repaint_bytes().len() >= 6 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(screen.lock().await.repaint_bytes(), b"onetwo".to_vec());
}
