//! Terminal geometry and the rendered-screen abstraction.
//!
//! The core owns a rendered screen per live session rather than a byte log.
//! Replaying historical bytes is wrong after a geometry change, and a
//! full-screen application's output stream cannot be seeked, so the only
//! correct way to answer "what is on screen right now" is to keep a live
//! model of the screen and ask it.

use std::num::NonZeroU16;

use serde::{Deserialize, Serialize};

/// Size of a session's terminal, in character cells.
///
/// Both dimensions are non-zero by construction. A zero dimension is a common
/// bug in client-side fit calculations during layout transitions, and making it
/// unrepresentable here means no downstream code has to re-validate it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalGeometry {
    /// Width in columns.
    pub columns: NonZeroU16,
    /// Height in rows.
    pub rows: NonZeroU16,
}

impl TerminalGeometry {
    /// Build a geometry, rejecting a zero in either dimension.
    pub fn new(columns: u16, rows: u16) -> Result<Self, InvalidGeometry> {
        match (NonZeroU16::new(columns), NonZeroU16::new(rows)) {
            (Some(columns), Some(rows)) => Ok(Self { columns, rows }),
            _ => Err(InvalidGeometry { columns, rows }),
        }
    }

    /// Width as a plain integer.
    pub fn columns(self) -> u16 {
        self.columns.get()
    }

    /// Height as a plain integer.
    pub fn rows(self) -> u16 {
        self.rows.get()
    }
}

impl Default for TerminalGeometry {
    /// A conventional 80x24, used until a client declares its real size.
    fn default() -> Self {
        Self {
            columns: NonZeroU16::new(80).expect("80 is non-zero"),
            rows: NonZeroU16::new(24).expect("24 is non-zero"),
        }
    }
}

/// A geometry was rejected because a dimension was zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("terminal geometry must be non-zero in both dimensions, got {columns}x{rows}")]
pub struct InvalidGeometry {
    /// The offending width.
    pub columns: u16,
    /// The offending height.
    pub rows: u16,
}

/// The current screen, in both the form a client repaints from and the form a
/// structured client reads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreenSnapshot {
    /// Geometry the snapshot was rendered at.
    pub geometry: TerminalGeometry,
    /// Plain-text rows, for clients that want structure rather than bytes.
    pub rows: Vec<String>,
}

/// A live model of one session's screen.
///
/// Implementations are injected: the core defines what a screen must be able to
/// do without taking on a dependency for how it is parsed.
pub trait ScreenModel: Send + Sync {
    /// Feed output from the runtime into the model.
    fn apply(&mut self, bytes: &[u8]);

    /// Re-render at a new geometry.
    fn resize(&mut self, geometry: TerminalGeometry);

    /// Read the current screen in structured form.
    fn snapshot(&self) -> ScreenSnapshot;

    /// Produce the byte sequence that restores everything a client should see.
    ///
    /// This is what a newly attached client receives, and it covers the
    /// scrollback as well as the visible screen: a terminal that comes back
    /// with only its current viewport has silently thrown away the
    /// conversation above it, and the person reattaching cannot tell whether
    /// their history is gone or merely out of reach.
    ///
    /// It must be the exact wire form, because deciding how to render
    /// structured cells into an escape sequence would be business logic, and
    /// that does not belong in an interface layer.
    fn repaint_bytes(&self) -> Vec<u8>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_geometry_is_accepted() {
        let g = TerminalGeometry::new(120, 40).expect("120x40 is valid");
        assert_eq!(g.columns(), 120);
        assert_eq!(g.rows(), 40);
    }

    #[test]
    fn zero_in_either_dimension_is_rejected() {
        assert!(TerminalGeometry::new(0, 40).is_err());
        assert!(TerminalGeometry::new(120, 0).is_err());
        assert!(TerminalGeometry::new(0, 0).is_err());
    }

    #[test]
    fn rejection_reports_the_offending_values() {
        let err = TerminalGeometry::new(0, 40).expect_err("zero width must fail");
        assert_eq!(err.columns, 0);
        assert_eq!(err.rows, 40);
    }

    #[test]
    fn default_is_the_conventional_size() {
        let g = TerminalGeometry::default();
        assert_eq!((g.columns(), g.rows()), (80, 24));
    }
}
