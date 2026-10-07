//! The WAV container around the raw PCM an `audio` job writes, and what that
//! PCM is metered as.
//!
//! The job writes headerless `s16le` (16 kHz, mono) to stdout after the
//! launcher's 44 zero bytes of headroom. The gateway writes its own header
//! into that headroom, so the WAV costs no second copy and no FFmpeg-written
//! header is ever parsed.
use bytes::BytesMut;

/// The canonical PCM WAV header's length.
pub const WAV_HEADER_BYTES: usize = 44;
/// 16 000 samples of 2 bytes, mono.
pub const PCM_BYTES_PER_SECOND: usize = 32_000;
const SAMPLE_RATE: u32 = 16_000;
const CHANNELS: u16 = 1;
const BITS_PER_SAMPLE: u16 = 16;
/// PCM format tag of the `fmt ` chunk.
const FORMAT_PCM: u16 = 1;
/// The `fmt ` chunk's body length for PCM.
const FMT_CHUNK_BYTES: u32 = 16;
/// The RIFF size counts everything after its own field except the data:
/// `WAVE`, the `fmt ` chunk (8 + 16) and the `data` chunk header (8).
const RIFF_OVERHEAD: u32 = 36;

/// Write the 44-byte header for `data_len` PCM bytes over the first 44 bytes
/// of `buffer`, which the launcher reserved as zero headroom.
///
/// # Panics
///
/// When `buffer` is shorter than the header: the launcher always reserves it
/// for an `audio` job, so a shorter buffer is a programming error.
pub fn write_wav_header(buffer: &mut BytesMut, data_len: u32) {
    let block_align = CHANNELS * (BITS_PER_SAMPLE / 8);
    let byte_rate = SAMPLE_RATE * u32::from(block_align);
    let header = &mut buffer[..WAV_HEADER_BYTES];
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&(RIFF_OVERHEAD + data_len).to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");
    header[12..16].copy_from_slice(b"fmt ");
    header[16..20].copy_from_slice(&FMT_CHUNK_BYTES.to_le_bytes());
    header[20..22].copy_from_slice(&FORMAT_PCM.to_le_bytes());
    header[22..24].copy_from_slice(&CHANNELS.to_le_bytes());
    header[24..28].copy_from_slice(&SAMPLE_RATE.to_le_bytes());
    header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    header[32..34].copy_from_slice(&block_align.to_le_bytes());
    header[34..36].copy_from_slice(&BITS_PER_SAMPLE.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_len.to_le_bytes());
}

/// The seconds `data_len` PCM bytes are metered as: exact, rounded up.
pub fn pcm_seconds(data_len: usize) -> u32 {
    u32::try_from(data_len.div_ceil(PCM_BYTES_PER_SECOND)).unwrap_or(u32::MAX)
}

/// The PCM bytes `seconds` of audio produce, which is also an `audio` job's
/// stdout cap.
pub fn pcm_bytes(seconds: u16) -> usize {
    usize::from(seconds) * PCM_BYTES_PER_SECOND
}
