//! The probe's `media` mode: well-formed output for each op, chosen by the
//! program name and the argv the job executes, so the self-test can run its
//! cases through the real sandbox without a real FFmpeg.
use image::{codecs::jpeg::JpegEncoder, ExtendedColorType};
use std::io::Write;

/// What `ffprobe` prints for a two-second clip with video and audio.
const PROBE_JSON: &str =
    r#"{"format":{"duration":"2.000"},"streams":[{"codec_type":"video"},{"codec_type":"audio"}]}"#;
/// Two seconds of 16 kHz mono PCM.
const PCM_BYTES: usize = 64_000;
/// The side of the grey JPEG a frame job writes.
const FRAME_SIDE: u32 = 8;

pub(crate) fn media(program: &str) {
    let args: Vec<String> = std::env::args().collect();
    let has = |flag: &str| args.iter().any(|arg| arg == flag);
    let mut stdout = std::io::stdout().lock();
    let written = match program {
        "ffprobe" => stdout.write_all(PROBE_JSON.as_bytes()),
        "ffmpeg" if has("-ss") => stdout.write_all(&grey_jpeg()),
        "ffmpeg" if has("-vn") => stdout.write_all(&[0; PCM_BYTES]),
        _ => std::process::exit(2),
    };
    if written.and_then(|()| stdout.flush()).is_err() {
        std::process::exit(1);
    }
}

/// An 8×8 mid-grey JPEG, encoded at runtime so no image is committed.
fn grey_jpeg() -> Vec<u8> {
    let pixels = [128_u8; (FRAME_SIDE * FRAME_SIDE) as usize];
    let mut jpeg = Vec::new();
    if JpegEncoder::new_with_quality(&mut jpeg, 90)
        .encode(&pixels, FRAME_SIDE, FRAME_SIDE, ExtendedColorType::L8)
        .is_err()
    {
        std::process::exit(1);
    }
    jpeg
}
