//! Folding a runtime's terminal bytes into a screen.
//!
//! Shared machinery rather than part of the trait, like `sandbox`: no kernel
//! is named here and none has to be. A pseudo-terminal emits an escape-encoded
//! stream, and turning that stream into "what is on screen right now" is one
//! problem with one answer, whichever kernel produced the bytes.
//!
//! It lives in this layer because this is the layer that owns the terminal.
//! The session core states what a screen is and holds one per live session; it
//! has no business knowing how a cursor-position sequence is parsed, and a
//! vt parser sitting next to the session model would be terminal-domain
//! machinery in the one crate that is meant to outlive any particular terminal.
//!
//! Nothing here is a trait. The core defines the contract a screen satisfies,
//! and the composition root is where that contract meets this implementation —
//! which is what keeps this layer free of any dependency on the layer above it.

use crate::TermSize;

/// Lines of scrollback retained behind the visible screen.
const SCROLLBACK: usize = 2000;

/// How much of the raw stream is kept so a client can be given the transcript.
///
/// Roughly the same span as the line scrollback above at a typical width, and
/// bounded because this is held per live session. Trimming loses the oldest
/// output, which is the part someone is least likely to be looking for.
const TRANSCRIPT_BYTES: usize = 256 * 1024;

/// A live screen, folded from the bytes a runtime's terminal produced.
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
pub struct TerminalScreen {
    parser: vt100::Parser,
    size: TermSize,
    transcript: std::collections::VecDeque<u8>,
}

impl TerminalScreen {
    /// Build a screen at the given size.
    pub fn new(size: TermSize) -> Self {
        Self {
            parser: vt100::Parser::new(size.rows, size.cols, SCROLLBACK),
            size,
            transcript: std::collections::VecDeque::new(),
        }
    }

    /// Fold output from the runtime into the screen.
    pub fn apply(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
        self.transcript.extend(bytes.iter().copied());
        self.trim_transcript();
    }

    /// Re-render at a new size.
    pub fn resize(&mut self, size: TermSize) {
        // Re-rendered before anything reads the screen. A snapshot taken at the
        // old size would paint the client's first frame at the wrong one, which
        // is exactly the corruption the attach sequence exists to prevent.
        self.parser.set_size(size.rows, size.cols);
        self.size = size;
    }

    /// The size this screen is currently rendered at.
    pub fn size(&self) -> TermSize {
        self.size
    }

    /// The visible screen as plain text, one entry per row.
    pub fn rows(&self) -> Vec<String> {
        let screen = self.parser.screen();
        (0..self.size.rows)
            .map(|row| screen.contents_between(row, 0, row, self.size.cols))
            .collect()
    }

    /// The byte sequence that restores everything a client should see.
    ///
    /// The stream as it happened. Replaying it reproduces the scrollback and
    /// the screen together, so a reattached client sees the conversation rather
    /// than only its last screenful — a distinction that is invisible until
    /// something goes wrong, and then is the whole story.
    ///
    /// When nothing has been retained there is still a screen worth painting,
    /// so that is reproduced directly instead.
    pub fn repaint_bytes(&self) -> Vec<u8> {
        if self.transcript.is_empty() {
            return self.parser.screen().contents_formatted();
        }
        self.transcript.iter().copied().collect()
    }

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

#[cfg(test)]
#[path = "terminal_tests.rs"]
mod terminal_tests;
