//! A terminal screen model backed by a real VT parser.
//!
//! Injected into the core so that the core defines *what* a screen must do
//! without taking a dependency on *how* escape sequences are parsed.

use layer2_core::{ScreenModel, ScreenSnapshot, TerminalGeometry};

/// Lines of scrollback retained behind the visible screen.
const SCROLLBACK: usize = 2000;

/// A screen model over a VT parser.
pub struct VtScreen {
    parser: vt100::Parser,
    geometry: TerminalGeometry,
}

impl VtScreen {
    /// Build a screen at the given geometry.
    pub fn new(geometry: TerminalGeometry) -> Self {
        Self {
            parser: vt100::Parser::new(geometry.rows(), geometry.columns(), SCROLLBACK),
            geometry,
        }
    }
}

impl ScreenModel for VtScreen {
    fn apply(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
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
        // The formatted contents are a self-contained escape sequence that
        // reproduces the screen, including attributes and cursor position.
        self.parser.screen().contents_formatted()
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
}
