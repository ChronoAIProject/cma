//! What a screen must do with the bytes a terminal produced.
//!
//! Moved here with the folding itself. The behaviour is unchanged; only the
//! layer that owns it is.

use super::{TerminalScreen, TRANSCRIPT_BYTES};
use crate::TermSize;

fn size(cols: u16, rows: u16) -> TermSize {
    TermSize { cols, rows }
}

#[test]
fn plain_output_lands_on_the_screen() {
    let mut s = TerminalScreen::new(size(80, 24));
    s.apply(b"hello world");
    let rows = s.rows();
    assert!(
        rows[0].contains("hello world"),
        "first row was {:?}",
        rows[0]
    );
}

#[test]
fn a_repaint_reproduces_the_screen_on_a_fresh_parser() {
    let mut original = TerminalScreen::new(size(80, 24));
    original.apply(b"restored content");

    let mut replayed = TerminalScreen::new(size(80, 24));
    replayed.apply(&original.repaint_bytes());

    assert_eq!(
        replayed.rows()[0].trim_end(),
        original.rows()[0].trim_end(),
        "a repaint must reconstruct the same screen"
    );
}

#[test]
fn resize_changes_the_reported_size() {
    let mut s = TerminalScreen::new(size(80, 24));
    assert_eq!(s.size().cols, 80);
    s.resize(size(120, 40));
    assert_eq!(s.size(), size(120, 40));
    assert_eq!(s.rows().len(), 40, "there must be one entry per row");
}

#[test]
fn escape_sequences_are_interpreted_rather_than_printed() {
    let mut s = TerminalScreen::new(size(20, 3));
    // Move to row 2 col 1, then write. A parser that ignored the sequence
    // would leave row 2 empty and put the text on row 1.
    s.apply(b"\x1b[2;1Hsecond");
    let rows = s.rows();
    assert!(rows[1].contains("second"), "rows were {rows:?}");
    assert!(!rows[0].contains("second"));
}

#[test]
fn a_cleared_screen_reports_empty_rows() {
    let mut s = TerminalScreen::new(size(20, 3));
    s.apply(b"noise everywhere");
    s.apply(b"\x1b[2J\x1b[H");
    let rows = s.rows();
    assert!(
        rows.iter().all(|r| r.trim().is_empty()),
        "expected a blank screen, got {rows:?}"
    );
}

#[test]
fn a_repaint_carries_the_lines_that_scrolled_off() {
    // The property this exists for: someone reattaching sees the conversation,
    // not just the last screenful. A viewport-only repaint is indistinguishable
    // from having lost the history.
    let mut s = TerminalScreen::new(size(20, 3));
    for line in 0..10 {
        s.apply(format!("line{line}\r\n").as_bytes());
    }

    let text = String::from_utf8_lossy(&s.repaint_bytes()).to_string();
    assert!(
        text.contains("line0"),
        "earliest line was dropped: {text:?}"
    );
    assert!(text.contains("line9"), "latest line was dropped: {text:?}");
}

#[test]
fn a_repaint_leaves_the_view_on_the_live_screen() {
    // Reading the transcript moves the parser's view. Leaving it moved would
    // mean the next output landed against a scrolled-back screen.
    let mut s = TerminalScreen::new(size(20, 3));
    for line in 0..10 {
        s.apply(format!("line{line}\r\n").as_bytes());
    }
    let _ = s.repaint_bytes();

    s.apply(b"after\r\n");
    let rows = s.rows().join("\n");
    assert!(
        rows.contains("after"),
        "new output did not reach the live screen: {rows:?}"
    );
}

#[test]
fn a_screen_with_no_history_repaints_only_itself() {
    // No spurious blank lines above a short session.
    let mut s = TerminalScreen::new(size(20, 5));
    s.apply(b"hello");
    let text = String::from_utf8_lossy(&s.repaint_bytes()).to_string();
    assert!(text.contains("hello"));
}

#[test]
fn a_replayed_repaint_reproduces_the_visible_screen() {
    // Restoring must be faithful, not approximate: the client renders these
    // bytes directly.
    let mut original = TerminalScreen::new(size(20, 4));
    for line in 0..8 {
        original.apply(format!("row{line}\r\n").as_bytes());
    }
    let bytes = original.repaint_bytes();

    let mut replayed = TerminalScreen::new(size(20, 4));
    replayed.apply(&bytes);
    assert_eq!(replayed.rows(), original.rows());
}

#[test]
fn the_transcript_is_trimmed_to_a_line_boundary() {
    // Cutting mid-escape would hand the client half a control sequence, which
    // renders as stray characters at the top of the restored view.
    let mut s = TerminalScreen::new(size(80, 24));
    for line in 0..20_000 {
        s.apply(format!("line {line} padding padding padding padding\r\n").as_bytes());
    }
    let bytes = s.repaint_bytes();
    assert!(
        bytes.len() <= TRANSCRIPT_BYTES + 1,
        "transcript grew unbounded"
    );
    assert!(!bytes.is_empty());
    // Replaying it must still land the latest output on the screen.
    let mut replayed = TerminalScreen::new(size(80, 24));
    replayed.apply(&bytes);
    let rows = replayed.rows().join("\n");
    assert!(
        rows.contains("19999"),
        "latest line missing after replay: {rows:?}"
    );
}
