//! A terminal screen model backed by a real VT parser.
//!
//! Injected into the core so that the core defines *what* a screen must do
//! without taking a dependency on *how* escape sequences are parsed.

use layer2_core::{ScreenModel, ScreenSnapshot, TerminalGeometry};

/// Lines of scrollback retained behind the visible screen.
const SCROLLBACK: usize = 2000;

/// How much of the raw stream is kept so a client can be given the transcript.
///
/// Roughly the same span as the line scrollback above at a typical width, and
/// bounded because this is held per live session. Trimming loses the oldest
/// output, which is the part someone is least likely to be looking for.
const TRANSCRIPT_BYTES: usize = 256 * 1024;

/// A screen model over a VT parser.
///
/// Two representations, because they answer different questions. The parser
/// tracks the *current screen* — what is where, at this geometry — which is
/// what a structured snapshot needs. The retained stream is the *transcript*,
/// which is what someone reattaching wants: not the last screenful, but the
/// conversation.
///
/// The transcript is kept as raw bytes rather than reconstructed from the
/// parser. Reconstruction would be the obvious approach and does not work:
/// vt100 exposes its retained lines only by moving its view, and moving it
/// further back than one screen height underflows inside the library. Replaying
/// the original bytes also has the better property — the client renders exactly
/// what it would have rendered live, rather than an approximation of it.
pub struct VtScreen {
    parser: vt100::Parser,
    geometry: TerminalGeometry,
    transcript: std::collections::VecDeque<u8>,
}

impl VtScreen {
    /// Build a screen at the given geometry.
    pub fn new(geometry: TerminalGeometry) -> Self {
        Self {
            parser: vt100::Parser::new(geometry.rows(), geometry.columns(), SCROLLBACK),
            geometry,
            transcript: std::collections::VecDeque::new(),
        }
    }
}

impl VtScreen {
    /// Keep the retained stream within its bound.
    ///
    /// Trimmed to a line boundary rather than to an exact byte count: cutting
    /// mid-escape would hand the client half a control sequence, which it would
    /// render as stray characters at the very top of the restored transcript.
    fn trim_transcript(&mut self) {
        if self.transcript.len() <= TRANSCRIPT_BYTES {
            return;
        }
        let excess = self.transcript.len() - TRANSCRIPT_BYTES;
        self.transcript.drain(..excess);
        // Advance to just past the next newline, so what remains starts at the
        // beginning of a line.
        if let Some(boundary) = self.transcript.iter().position(|b| *b == b'\n') {
            self.transcript.drain(..=boundary);
        }
    }
}

impl ScreenModel for VtScreen {
    fn apply(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        self.transcript.extend(bytes.iter().copied());
        self.trim_transcript();
    }

    fn resize(&mut self, geometry: TerminalGeometry) {
        // Re-render at the new size before anything reads the screen. A
        // snapshot taken at the old geometry would paint the client's first
        // frame at the wrong size, which is exactly the corruption the attach
        // sequence exists to prevent.
        self.parser.set_size(geometry.rows(), geometry.columns());
        self.geometry = geometry;
    }

    fn snapshot(&self) -> ScreenSnapshot {
        let screen = self.parser.screen();
        let rows = (0..self.geometry.rows())
            .map(|r| screen.contents_between(r, 0, r, self.geometry.columns()))
            .collect();
        ScreenSnapshot {
            geometry: self.geometry,
            rows,
        }
    }

    fn repaint_bytes(&self) -> Vec<u8> {
        // The stream as it happened. Replaying it reproduces the scrollback and
        // the screen together, so a reattached client sees the conversation
        // rather than only its last screenful — a distinction that is invisible
        // until something goes wrong, and then is the whole story.
        //
        // When nothing has been retained there is still a screen worth
        // painting, so fall back to reproducing it directly.
        if self.transcript.is_empty() {
            return self.parser.screen().contents_formatted();
        }
        self.transcript.iter().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(cols: u16, rows: u16) -> TerminalGeometry {
        TerminalGeometry::new(cols, rows).expect("valid geometry")
    }

    #[test]
    fn plain_output_lands_on_the_screen() {
        let mut s = VtScreen::new(geometry(80, 24));
        s.apply(b"hello world");
        let snap = s.snapshot();
        assert!(
            snap.rows[0].contains("hello world"),
            "first row was {:?}",
            snap.rows[0]
        );
    }

    #[test]
    fn a_repaint_reproduces_the_screen_on_a_fresh_parser() {
        let mut original = VtScreen::new(geometry(80, 24));
        original.apply(b"restored content");

        let mut replayed = VtScreen::new(geometry(80, 24));
        replayed.apply(&original.repaint_bytes());

        assert_eq!(
            replayed.snapshot().rows[0].trim_end(),
            original.snapshot().rows[0].trim_end(),
            "a repaint must reconstruct the same screen"
        );
    }

    #[test]
    fn resize_changes_the_reported_geometry() {
        let mut s = VtScreen::new(geometry(80, 24));
        assert_eq!(s.snapshot().geometry.columns(), 80);
        s.resize(geometry(120, 40));
        let snap = s.snapshot();
        assert_eq!(snap.geometry.columns(), 120);
        assert_eq!(snap.geometry.rows(), 40);
        assert_eq!(snap.rows.len(), 40, "snapshot must have one entry per row");
    }

    #[test]
    fn escape_sequences_are_interpreted_rather_than_printed() {
        let mut s = VtScreen::new(geometry(20, 3));
        // Move to row 2 col 1, then write. A parser that ignored the sequence
        // would leave row 2 empty and put the text on row 1.
        s.apply(b"\x1b[2;1Hsecond");
        let snap = s.snapshot();
        assert!(snap.rows[1].contains("second"), "rows were {:?}", snap.rows);
        assert!(!snap.rows[0].contains("second"));
    }

    #[test]
    fn a_cleared_screen_reports_empty_rows() {
        let mut s = VtScreen::new(geometry(20, 3));
        s.apply(b"noise everywhere");
        s.apply(b"\x1b[2J\x1b[H");
        let snap = s.snapshot();
        assert!(
            snap.rows.iter().all(|r| r.trim().is_empty()),
            "expected a blank screen, got {:?}",
            snap.rows
        );
    }

    #[test]
    fn a_repaint_carries_the_lines_that_scrolled_off() {
        // The property this exists for: someone reattaching sees the
        // conversation, not just the last screenful. A viewport-only repaint
        // is indistinguishable from having lost the history.
        let mut s = VtScreen::new(geometry(20, 3));
        for line in 0..10 {
            s.apply(format!("line{line}\r\n").as_bytes());
        }

        let repaint = s.repaint_bytes();
        let text = String::from_utf8_lossy(&repaint);
        assert!(
            text.contains("line0"),
            "earliest line was dropped: {text:?}"
        );
        assert!(text.contains("line9"), "latest line was dropped: {text:?}");
    }

    #[test]
    fn a_repaint_leaves_the_view_on_the_live_screen() {
        // Reading the transcript moves the parser's view. Leaving it moved
        // would mean the next output landed against a scrolled-back screen.
        let mut s = VtScreen::new(geometry(20, 3));
        for line in 0..10 {
            s.apply(format!("line{line}\r\n").as_bytes());
        }
        let _ = s.repaint_bytes();

        s.apply(b"after\r\n");
        let rows = s.snapshot().rows.join("\n");
        assert!(
            rows.contains("after"),
            "new output did not reach the live screen: {rows:?}"
        );
    }

    #[test]
    fn a_screen_with_no_history_repaints_only_itself() {
        // No spurious blank lines above a short session.
        let mut s = VtScreen::new(geometry(20, 5));
        s.apply(b"hello");
        let text = String::from_utf8_lossy(&s.repaint_bytes()).to_string();
        assert!(text.contains("hello"));
    }

    #[test]
    fn a_replayed_repaint_reproduces_the_visible_screen() {
        // Restoring must be faithful, not approximate: the client renders
        // these bytes directly.
        let mut original = VtScreen::new(geometry(20, 4));
        for line in 0..8 {
            original.apply(format!("row{line}\r\n").as_bytes());
        }
        let bytes = original.repaint_bytes();

        let mut replayed = VtScreen::new(geometry(20, 4));
        replayed.apply(&bytes);
        assert_eq!(replayed.snapshot().rows, original.snapshot().rows);
    }

    #[test]
    fn the_transcript_is_trimmed_to_a_line_boundary() {
        // Cutting mid-escape would hand the client half a control sequence,
        // which renders as stray characters at the top of the restored view.
        let mut s = VtScreen::new(geometry(80, 24));
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
        let mut replayed = VtScreen::new(geometry(80, 24));
        replayed.apply(&bytes);
        let rows = replayed.snapshot().rows.join("\n");
        assert!(
            rows.contains("19999"),
            "latest line missing after replay: {rows:?}"
        );
    }
}
