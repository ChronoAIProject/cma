//! The sandboxed FFmpeg transformer's settings: where its binaries live and
//! how many keyframes a video yields.
//!
//! An empty `CMAEG_MEDIA_FFMPEG_DIR` (the default) leaves the transformer
//! off: media keeps platform thumbnails and direct speech-to-text uploads.
//! The binary directory itself is checked on the filesystem at startup, by
//! the transformer's setup, only once the transformer is configured. The
//! watchdog's period and threshold and the single transform slot are code
//! constants, not keys.
use super::{
    media::{bounded, MediaConfig, CMAEG_MEDIA_ENABLED, CMAEG_MEDIA_INFLIGHT_BYTES},
    stt::{SttConfig, CMAEG_STT_MAX_BYTES},
    ConfigError, CMAEG_MEDIA_MAX_BYTES,
};
use std::path::PathBuf;

pub const CMAEG_MEDIA_FFMPEG_DIR: &str = "CMAEG_MEDIA_FFMPEG_DIR";
pub const CMAEG_MEDIA_VIDEO_FRAMES: &str = "CMAEG_MEDIA_VIDEO_FRAMES";

/// Default, minimum and maximum keyframes per video.
const VIDEO_FRAMES: (u64, u64, u64) = (6, 1, 6);
/// Whether this build has the job sandbox at all.
const SANDBOX_PLATFORM: bool = cfg!(all(target_os = "linux", target_arch = "x86_64"));

/// The transformer's settings, loaded once at startup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransformerConfig {
    /// The directory holding `ffmpeg` and `ffprobe`; `None` while the key is
    /// empty.
    pub ffmpeg_dir: Option<PathBuf>,
    /// Keyframes per video, `1..=6`.
    pub video_frames: u8,
}

impl Default for TransformerConfig {
    fn default() -> Self {
        Self {
            ffmpeg_dir: None,
            video_frames: VIDEO_FRAMES.0 as u8,
        }
    }
}

impl TransformerConfig {
    /// Read both keys through `lookup`, so tests need not mutate the process
    /// environment.
    pub fn from_lookup(
        lookup: impl Fn(&'static str) -> Result<Option<String>, ConfigError>,
    ) -> Result<Self, ConfigError> {
        let (default, min, max) = VIDEO_FRAMES;
        let video_frames = bounded(
            CMAEG_MEDIA_VIDEO_FRAMES,
            lookup(CMAEG_MEDIA_VIDEO_FRAMES)?.as_deref(),
            default,
            min,
            max,
        )?;
        let ffmpeg_dir = match lookup(CMAEG_MEDIA_FFMPEG_DIR)?.as_deref() {
            None | Some("") => None,
            Some(value) => Some(directory(value)?),
        };
        Ok(Self {
            ffmpeg_dir,
            // Bounded to at most 6 just above.
            video_frames: u8::try_from(video_frames).unwrap_or(u8::MAX),
        })
    }

    /// The transformer runs only when a binary directory is set and inbound
    /// media is on.
    pub fn configured(&self, media: &MediaConfig) -> bool {
        self.ffmpeg_dir.is_some() && media.enabled
    }

    /// Cross-key rules, checked once media and speech-to-text are loaded.
    ///
    /// A configured transformer holds a fetched original and an audio job's
    /// whole PCM output at once, so the in-flight budget must fit both.
    pub fn check(&self, media: &MediaConfig, stt: &SttConfig) -> Result<(), ConfigError> {
        if self.ffmpeg_dir.is_some() && !media.enabled {
            tracing::warn!(
                key = CMAEG_MEDIA_FFMPEG_DIR,
                "the media transformer is ignored while {CMAEG_MEDIA_ENABLED} is false"
            );
        }
        if !self.configured(media) {
            return Ok(());
        }
        let needed = media.max_bytes.saturating_add(stt.max_bytes);
        if media.inflight_bytes < needed {
            return Err(ConfigError::InvalidMedia {
                detail: format!(
                    "{CMAEG_MEDIA_INFLIGHT_BYTES} ({}) must be at least {CMAEG_MEDIA_MAX_BYTES} ({}) \
                     + {CMAEG_STT_MAX_BYTES} ({}) while the media transformer is configured",
                    media.inflight_bytes, media.max_bytes, stt.max_bytes
                ),
            });
        }
        tracing::info!(
            video_frames = self.video_frames,
            "media transformer configured"
        );
        Ok(())
    }
}

/// A non-empty binary directory: absolute, and only where the sandbox exists.
fn directory(value: &str) -> Result<PathBuf, ConfigError> {
    if !SANDBOX_PLATFORM {
        return Err(ConfigError::InvalidMedia {
            detail: format!(
                "{CMAEG_MEDIA_FFMPEG_DIR} must be empty: the media job sandbox requires linux x86_64"
            ),
        });
    }
    let path = PathBuf::from(value);
    if !path.is_absolute() {
        return Err(ConfigError::InvalidMedia {
            detail: format!("{CMAEG_MEDIA_FFMPEG_DIR} must be an absolute path"),
        });
    }
    Ok(path)
}

/// Read the transformer's keys from the process environment.
pub(super) fn config_from_env() -> Result<TransformerConfig, ConfigError> {
    TransformerConfig::from_lookup(super::media::read_env)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn read(pairs: &[(&'static str, &str)]) -> Result<TransformerConfig, ConfigError> {
        let values: HashMap<_, _> = pairs.iter().copied().collect();
        TransformerConfig::from_lookup(|key| Ok(values.get(key).map(|value| (*value).to_owned())))
    }

    fn detail(error: ConfigError) -> String {
        match error {
            ConfigError::InvalidMedia { detail } => detail,
            other => panic!("expected an invalid media configuration, got {other}"),
        }
    }

    fn media(enabled: bool) -> MediaConfig {
        MediaConfig {
            enabled,
            ..MediaConfig::default()
        }
    }

    #[test]
    fn the_default_is_off_with_six_frames() {
        let config = read(&[]).unwrap();
        assert_eq!(config, TransformerConfig::default());
        assert_eq!(config.ffmpeg_dir, None);
        assert_eq!(config.video_frames, 6);
        assert_eq!(
            read(&[(CMAEG_MEDIA_FFMPEG_DIR, "")]).unwrap().ffmpeg_dir,
            None
        );
        assert!(!config.configured(&media(true)));
    }

    #[test]
    fn video_frames_lie_between_one_and_six() {
        assert_eq!(
            read(&[(CMAEG_MEDIA_VIDEO_FRAMES, "1")])
                .unwrap()
                .video_frames,
            1
        );
        for value in ["0", "7", "", "six", "-1"] {
            assert_eq!(
                detail(read(&[(CMAEG_MEDIA_VIDEO_FRAMES, value)]).unwrap_err()),
                "CMAEG_MEDIA_VIDEO_FRAMES must be an unsigned decimal from 1 to 6",
                "{value:?}"
            );
        }
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn a_directory_must_be_absolute() {
        assert_eq!(
            detail(read(&[(CMAEG_MEDIA_FFMPEG_DIR, "opt/ffmpeg/bin")]).unwrap_err()),
            "CMAEG_MEDIA_FFMPEG_DIR must be an absolute path"
        );
        let config = read(&[(CMAEG_MEDIA_FFMPEG_DIR, "/opt/ffmpeg/bin")]).unwrap();
        assert_eq!(config.ffmpeg_dir, Some(PathBuf::from("/opt/ffmpeg/bin")));
        assert!(config.configured(&media(true)));
    }

    #[cfg(not(all(target_os = "linux", target_arch = "x86_64")))]
    #[test]
    fn a_directory_is_refused_without_the_sandbox() {
        for value in ["/opt/ffmpeg/bin", "opt/ffmpeg/bin"] {
            assert_eq!(
                detail(read(&[(CMAEG_MEDIA_FFMPEG_DIR, value)]).unwrap_err()),
                "CMAEG_MEDIA_FFMPEG_DIR must be empty: the media job sandbox requires linux x86_64"
            );
        }
    }

    #[test]
    fn a_configured_transformer_needs_inflight_room_for_an_original_and_its_pcm() {
        let configured = TransformerConfig {
            ffmpeg_dir: Some(PathBuf::from("/opt/ffmpeg/bin")),
            video_frames: 6,
        };
        let stt = SttConfig::default();
        // The defaults: 64 MiB >= 20 MiB + 20 MiB.
        assert!(configured.check(&media(true), &stt).is_ok());
        let tight = MediaConfig {
            inflight_bytes: 41_943_039,
            ..media(true)
        };
        assert_eq!(
            detail(configured.check(&tight, &stt).unwrap_err()),
            "CMAEG_MEDIA_INFLIGHT_BYTES (41943039) must be at least CMAEG_MEDIA_MAX_BYTES \
             (20971520) + CMAEG_STT_MAX_BYTES (20971520) while the media transformer is configured"
        );
        let exact = MediaConfig {
            inflight_bytes: 41_943_040,
            ..media(true)
        };
        assert!(configured.check(&exact, &stt).is_ok());
        // Ignored, with a warning, while media is off: nothing to check.
        assert!(!configured.configured(&tight_off()));
        assert!(configured.check(&tight_off(), &stt).is_ok());
    }

    fn tight_off() -> MediaConfig {
        MediaConfig {
            inflight_bytes: 1,
            ..media(false)
        }
    }
}
