//! Bounded presentation profiles. Bytes are validated and forwarded unchanged.
use crate::media::{
    outbound::PreparedFile,
    outbound_profiles::{ogg, video},
};
use cmaeg_core::message::NativeMediaKind;

pub(super) fn prepare(kind: NativeMediaKind, file: &mut PreparedFile) -> bool {
    let (mime, extension) = match kind {
        NativeMediaKind::Voice if ogg::mono_opus(&file.bytes) => ("audio/ogg", "ogg"),
        NativeMediaKind::Animation if gif(&file.bytes) => ("image/gif", "gif"),
        NativeMediaKind::Animation if video::silent_mp4(&file.bytes) => ("video/mp4", "mp4"),
        NativeMediaKind::Sticker
            if file.caption.is_none()
                && crate::whatsapp::outbound::sticker::inspect(&file.bytes) == Some(false) =>
        {
            ("image/webp", "webp")
        }
        _ => return false,
    };
    if file.mime != "application/octet-stream" && file.mime != mime {
        return false;
    }
    if let Some((_, supplied)) = file.filename.rsplit_once('.') {
        if !supplied.eq_ignore_ascii_case(extension) {
            return false;
        }
    } else {
        file.filename.push('.');
        file.filename.push_str(extension);
    }
    file.mime = mime.into();
    true
}

/// Parse bounded GIF blocks without decoding pixels or allocating per frame.
/// Complete compressed-image validity remains the provider's responsibility.
fn gif(bytes: &[u8]) -> bool {
    fn check(bytes: &[u8]) -> Option<()> {
        let header = bytes.get(..13)?;
        if header[..6] != *b"GIF87a" && header[..6] != *b"GIF89a" {
            return None;
        }
        let width = u16::from_le_bytes(header[6..8].try_into().ok()?);
        let height = u16::from_le_bytes(header[8..10].try_into().ok()?);
        if width == 0 || height == 0 || width > 1920 || height > 1080 {
            return None;
        }
        let mut cursor = 13;
        if header[10] & 0x80 != 0 {
            cursor += 3 * (1usize << ((header[10] & 7) + 1));
        }
        bytes.get(..cursor)?;
        let mut frames = 0;
        loop {
            match *bytes.get(cursor)? {
                0x3b => return (frames > 0 && cursor + 1 == bytes.len()).then_some(()),
                0x21 => {
                    bytes.get(cursor + 1)?;
                    cursor += 2;
                    blocks(bytes, &mut cursor)?;
                }
                0x2c => {
                    let frame = bytes.get(cursor + 1..cursor + 10)?;
                    let x = u16::from_le_bytes(frame[..2].try_into().ok()?) as u32;
                    let y = u16::from_le_bytes(frame[2..4].try_into().ok()?) as u32;
                    let w = u16::from_le_bytes(frame[4..6].try_into().ok()?) as u32;
                    let h = u16::from_le_bytes(frame[6..8].try_into().ok()?) as u32;
                    if w == 0
                        || h == 0
                        || x + w > width as u32
                        || y + h > height as u32
                        || frame[8] & 0x18 != 0
                    {
                        return None;
                    }
                    cursor += 10;
                    if frame[8] & 0x80 != 0 {
                        cursor += 3 * (1usize << ((frame[8] & 7) + 1));
                    }
                    if !(2..=8).contains(bytes.get(cursor)?) {
                        return None;
                    }
                    cursor += 1;
                    if *bytes.get(cursor)? == 0 {
                        return None;
                    }
                    blocks(bytes, &mut cursor)?;
                    frames += 1;
                    if frames > 256 {
                        return None;
                    }
                }
                _ => return None,
            }
        }
    }
    check(bytes).is_some()
}

fn blocks(bytes: &[u8], cursor: &mut usize) -> Option<()> {
    loop {
        let size = *bytes.get(*cursor)? as usize;
        *cursor += 1;
        if size == 0 {
            return Some(());
        }
        bytes.get(*cursor..*cursor + size)?;
        *cursor += size;
    }
}

#[cfg(test)]
#[path = "media_native_presentation_tests.rs"]
mod tests;
