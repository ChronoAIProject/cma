//! The part of ffprobe's JSON the keyframe set uses: the container duration.
//!
//! The types are strict where they are read and ignore every other field,
//! because ffprobe also emits sections such as `programs` and
//! `stream_groups` whose shape varies by version.
use serde::Deserialize;

/// The longest duration a frame position may name: one day.
pub const MAX_DURATION_SECONDS: f64 = 86_400.0;

/// `ffprobe -show_entries format=duration:stream=codec_type -of json`.
#[derive(Debug, Default, Deserialize)]
pub struct ProbeOutput {
    #[serde(default)]
    pub format: Option<ProbeFormat>,
    #[serde(default)]
    pub streams: Vec<ProbeStream>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ProbeFormat {
    /// ffprobe prints numbers in this section as strings.
    #[serde(default)]
    pub duration: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
pub struct ProbeStream {
    #[serde(default)]
    pub codec_type: Option<String>,
}

impl ProbeOutput {
    /// The probe's stdout, or `None` when it is not this JSON shape.
    pub fn parse(stdout: &[u8]) -> Option<Self> {
        serde_json::from_slice(stdout).ok()
    }

    /// The container duration in seconds when it is a finite number in
    /// `(0, 86400]`; anything else is an unknown duration.
    pub fn duration_seconds(&self) -> Option<f64> {
        let value = self.format.as_ref()?.duration.as_deref()?;
        value
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
            .filter(|seconds| *seconds <= MAX_DURATION_SECONDS)
    }
}
