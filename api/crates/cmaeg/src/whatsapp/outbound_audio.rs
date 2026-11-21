//! Bounded container/codec eligibility checks, without decoding or subprocesses.
//! Payload decodability and delivery acceptance remain Meta's responsibility.
#[path = "outbound_audio/mp4.rs"]
mod mp4;
use crate::media::outbound_profiles::ogg;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Format {
    Aac,
    Amr,
    Mp3,
    Mp4,
    Ogg,
}
impl Format {
    pub fn mime(self) -> &'static str {
        match self {
            Self::Aac => "audio/aac",
            Self::Amr => "audio/amr",
            Self::Mp3 => "audio/mpeg",
            Self::Mp4 => "audio/mp4",
            Self::Ogg => "audio/ogg",
        }
    }
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Aac => &["aac"],
            Self::Amr => &["amr"],
            Self::Mp3 => &["mp3"],
            Self::Mp4 => &["m4a"],
            Self::Ogg => &["ogg"],
        }
    }
}

pub(super) fn inspect(bytes: &[u8]) -> Option<Format> {
    if bytes.is_empty() || bytes.len() as u64 > crate::media::outbound::MAX_OUTBOUND_BYTES {
        return None;
    }
    if bytes.starts_with(b"#!AMR") {
        return amr(bytes).then_some(Format::Amr);
    }
    if bytes.starts_with(b"OggS") {
        return ogg::mono_opus(bytes).then_some(Format::Ogg);
    }
    if bytes.get(4..8) == Some(b"ftyp") {
        return mp4::aac_audio(bytes).then_some(Format::Mp4);
    }
    let frames = without_id3(bytes)?;
    if adts(frames) {
        Some(Format::Aac)
    } else if mp3(frames) {
        Some(Format::Mp3)
    } else {
        None
    }
}

/// ID3 is bounded metadata, never another URI or input for a decoder.
fn without_id3(bytes: &[u8]) -> Option<&[u8]> {
    if !bytes.starts_with(b"ID3") {
        return Some(bytes);
    }
    let header = bytes.get(..10)?;
    if !matches!(header[3], 2..=4)
        || header[4] == 0xff
        || header[6..].iter().any(|byte| byte & 0x80 != 0)
    {
        return None;
    }
    let length = header[6..]
        .iter()
        .fold(0usize, |length, byte| (length << 7) | usize::from(*byte));
    let footer = header[3] == 4 && header[5] & 0x10 != 0;
    let end = 10usize.checked_add(length)?;
    if footer && bytes.get(end..end.checked_add(3)?) != Some(b"3DI") {
        return None;
    }
    bytes.get(end.checked_add(if footer { 10 } else { 0 })?..)
}

/// AAC-LC in ADTS: every frame must fit and keep one sample/channel configuration.
fn adts(mut bytes: &[u8]) -> bool {
    let mut config = None;
    let mut count = 0;
    while !bytes.is_empty() {
        let Some(header) = bytes.get(..7) else {
            return false;
        };
        if header[0] != 0xff || header[1] & 0xf6 != 0xf0 || header[2] >> 6 != 1 {
            return false;
        }
        let sample_index = (header[2] >> 2) & 0x0f;
        let channels = ((header[2] & 1) << 2) | (header[3] >> 6);
        if sample_index > 12 || channels == 0 {
            return false;
        }
        let current = (header[1] & 8, sample_index, channels);
        if config.is_some_and(|value| value != current) {
            return false;
        }
        config = Some(current);
        let length = (usize::from(header[3] & 3) << 11)
            | (usize::from(header[4]) << 3)
            | usize::from(header[5] >> 5);
        let overhead = if header[1] & 1 == 0 { 9 } else { 7 };
        if length <= overhead {
            return false;
        }
        let Some(rest) = bytes.get(length..) else {
            return false;
        };
        bytes = rest;
        count += 1;
    }
    count > 0
}

/// MPEG-1/2/2.5 Layer III, including VBR, with bounded leading/trailing ID3.
fn mp3(mut bytes: &[u8]) -> bool {
    const MPEG1: [usize; 16] = [
        0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 0,
    ];
    const MPEG2: [usize; 16] = [
        0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160, 0,
    ];
    let mut config = None;
    let mut count = 0;
    while !bytes.is_empty() {
        if count > 0 && bytes.len() == 128 && bytes.starts_with(b"TAG") {
            return true;
        }
        let Some(header) = bytes.get(..4) else {
            return false;
        };
        let version = (header[1] >> 3) & 3;
        let frequency = (header[2] >> 2) & 3;
        if header[0] != 0xff || header[1] & 0xe6 != 0xe2 || version == 1 || frequency == 3 {
            return false;
        }
        let current = (version, frequency);
        if config.is_some_and(|value| value != current) {
            return false;
        }
        config = Some(current);
        let rate = [44100, 48000, 32000][usize::from(frequency)]
            / match version {
                3 => 1,
                2 => 2,
                _ => 4,
            };
        let bitrate = (if version == 3 { MPEG1 } else { MPEG2 })[usize::from(header[2] >> 4)];
        if bitrate == 0 {
            return false;
        }
        let length = (if version == 3 { 144000 } else { 72000 }) * bitrate / rate
            + usize::from((header[2] >> 1) & 1);
        if length <= 4 {
            return false;
        }
        let Some(rest) = bytes.get(length..) else {
            return false;
        };
        bytes = rest;
        count += 1;
    }
    count > 0
}

/// Single-channel AMR narrowband storage frames. Wideband has a different MIME
/// type (audio/amr-wb), outside Meta's documented audio/amr allowlist.
fn amr(bytes: &[u8]) -> bool {
    let Some(mut bytes) = bytes.strip_prefix(b"#!AMR\n") else {
        return false;
    };
    let sizes = [12, 13, 15, 17, 19, 20, 26, 31, 5];
    let mut speech = false;
    while let Some(&header) = bytes.first() {
        if header & 0x83 != 0 {
            return false;
        }
        let frame_type = usize::from((header >> 3) & 15);
        let size = if let Some(&size) = sizes.get(frame_type) {
            speech |= frame_type < sizes.len() - 1;
            size
        } else if frame_type == 15 {
            0
        } else {
            return false;
        };
        let Some(rest) = bytes.get(1 + size..) else {
            return false;
        };
        bytes = rest;
    }
    speech
}

#[cfg(test)]
#[path = "outbound_audio/tests.rs"]
pub(crate) mod tests;
