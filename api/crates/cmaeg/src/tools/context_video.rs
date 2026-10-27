use serde::Serialize;
use utoipa::ToSchema;

/// Bounded structural video profile; full decoding and provider acceptance remain separate.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct NativeVideoCapabilities {
    pub video_codec: String,
    pub video_profile: String,
    pub audio_codec: String,
    pub max_audio_tracks: u32,
    pub max_width: u32,
    pub max_height: u32,
    pub max_frame_rate: u32,
    pub max_duration_ms: u32,
    /// Total samples across video and optional audio tracks.
    pub max_samples: usize,
    pub progressive_only: bool,
    pub max_slices_per_frame: u32,
    pub fast_start_required: bool,
}
impl NativeVideoCapabilities {
    pub(super) fn whatsapp() -> Self {
        use crate::whatsapp::outbound::video;
        Self {
            video_codec: "h264".into(),
            video_profile: "baseline".into(),
            audio_codec: "aac_lc".into(),
            max_audio_tracks: 1,
            max_width: video::MAX_WIDTH,
            max_height: video::MAX_HEIGHT,
            max_frame_rate: video::MAX_FRAME_RATE,
            max_duration_ms: video::MAX_DURATION_MS,
            max_samples: video::MAX_SAMPLES,
            progressive_only: true,
            max_slices_per_frame: 1,
            fast_start_required: true,
        }
    }
}
