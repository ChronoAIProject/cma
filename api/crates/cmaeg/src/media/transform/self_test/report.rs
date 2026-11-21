//! The self-test's report: one line per case and op, carrying an outcome,
//! counts and a duration only.
use crate::media::transform::{outcome::TransformOutcome, TransformError};
use std::fmt;

/// What a case asked of the transformer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseOp {
    Keyframes,
    Audio,
}

impl CaseOp {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Keyframes => "keyframes",
            Self::Audio => "audio",
        }
    }
}

/// How one case ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseOutcome {
    Ok,
    /// A job ended this way: the set's failure, or an outcome no case may
    /// see.
    Job(TransformOutcome),
    /// The set's deadline ended it before any frame.
    NoFrames,
    /// The transformer refused the request.
    Unavailable,
    /// The probe reported no usable duration.
    ProbeInvalid,
    /// More frames than requested, or fewer than an exact case requires.
    FrameCount,
    /// A frame larger than a preview, or one that does not decode.
    FrameInvalid,
    /// PCM that is short or odd, or a header unlike the one `pcm.rs` writes.
    PcmInvalid,
    /// The fixture could not be read.
    FixtureUnreadable,
}

impl CaseOutcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Job(outcome) => outcome.as_str(),
            Self::NoFrames => "no_frames",
            Self::Unavailable => "unavailable",
            Self::ProbeInvalid => "probe_invalid",
            Self::FrameCount => "frame_count",
            Self::FrameInvalid => "frame_invalid",
            Self::PcmInvalid => "pcm_invalid",
            Self::FixtureUnreadable => "fixture_unreadable",
        }
    }

    pub(super) fn of(error: TransformError) -> Self {
        match error {
            TransformError::Job { outcome, .. } => Self::Job(outcome),
            TransformError::NoFrames => Self::NoFrames,
            TransformError::Unavailable => Self::Unavailable,
        }
    }
}

/// One report line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseLine {
    pub file: &'static str,
    pub op: CaseOp,
    pub outcome: CaseOutcome,
    pub frames: usize,
    pub pcm_bytes: usize,
    pub ms: u128,
}

impl fmt::Display for CaseLine {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "media-self-test case={} op={} outcome={} frames={} pcm_bytes={} ms={}",
            self.file,
            self.op.as_str(),
            self.outcome.as_str(),
            self.frames,
            self.pcm_bytes,
            self.ms
        )
    }
}

/// Every case's line, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    pub lines: Vec<CaseLine>,
}

impl Report {
    /// Some case ran and every one passed.
    pub fn passed(&self) -> bool {
        !self.lines.is_empty()
            && self
                .lines
                .iter()
                .all(|line| line.outcome == CaseOutcome::Ok)
    }
}
