//! Where the core's idea of a screen meets the layer that folds one.
//!
//! The core states what a screen must be able to do and holds one per live
//! session; Layer 3 owns the terminal and knows how an escape-encoded stream
//! becomes a screen. Neither may depend on the other's contract — Layer 3 must
//! not know the core exists — so the two are joined here, in the composition
//! root, which is the one place that is allowed to know both.
//!
//! Nothing is decided here. This is a translation of geometry into size and of
//! rows into a snapshot, and if it ever grows a judgement it belongs in one of
//! the layers it joins.

use layer2_core::{ScreenModel, ScreenSnapshot, TerminalGeometry};
use layer3_adapter::terminal::TerminalScreen;

/// The core's screen model, backed by Layer 3's terminal folding.
pub struct VtScreen {
    screen: TerminalScreen,
    /// Held alongside because the core expresses a screen's size as a geometry
    /// — a type that cannot be zero in either dimension — and Layer 3 speaks in
    /// plain cell counts. Keeping the validated value avoids reconstructing it,
    /// and with it the question of what to do when reconstruction fails.
    geometry: TerminalGeometry,
}

impl VtScreen {
    /// Build a screen at the given geometry.
    pub fn new(geometry: TerminalGeometry) -> Self {
        Self {
            screen: TerminalScreen::new(size_of(geometry)),
            geometry,
        }
    }
}

/// The core's geometry, in the cell counts Layer 3 speaks.
fn size_of(geometry: TerminalGeometry) -> layer3_adapter::TermSize {
    layer3_adapter::TermSize {
        cols: geometry.columns(),
        rows: geometry.rows(),
    }
}

impl ScreenModel for VtScreen {
    fn apply(&mut self, bytes: &[u8]) {
        self.screen.apply(bytes);
    }

    fn resize(&mut self, geometry: TerminalGeometry) {
        self.screen.resize(size_of(geometry));
        self.geometry = geometry;
    }

    fn snapshot(&self) -> ScreenSnapshot {
        ScreenSnapshot {
            geometry: self.geometry,
            rows: self.screen.rows(),
        }
    }

    fn repaint_bytes(&self) -> Vec<u8> {
        self.screen.repaint_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(cols: u16, rows: u16) -> TerminalGeometry {
        TerminalGeometry::new(cols, rows).expect("valid geometry")
    }

    #[test]
    fn output_reaches_the_screen_through_the_contract() {
        // What this file is for: the core's trait, satisfied by Layer 3's
        // folding. How the folding itself behaves is tested where it lives.
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
    fn a_snapshot_reports_the_geometry_it_was_rendered_at() {
        // The geometry, not a reconstruction of it: a client paints its first
        // frame from this, and a wrong size there is corruption that does not
        // self-heal until the next full repaint.
        let mut s = VtScreen::new(geometry(80, 24));
        assert_eq!(s.snapshot().geometry.columns(), 80);
        s.resize(geometry(120, 40));
        let snap = s.snapshot();
        assert_eq!((snap.geometry.columns(), snap.geometry.rows()), (120, 40));
        assert_eq!(snap.rows.len(), 40, "snapshot must have one entry per row");
    }

    #[test]
    fn a_repaint_carries_what_scrolled_off_as_well_as_the_screen() {
        let mut s = VtScreen::new(geometry(20, 3));
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
}
