//! Bounded WebP container eligibility; no compressed pixels are decoded here.
//! Meta remains responsible for complete payload validity and client acceptance.
pub(crate) const MAX_STATIC_BYTES: usize = 100_000;
pub(crate) const MAX_ANIMATED_BYTES: usize = 500_000;
pub(crate) const SIDE: u32 = 512;
pub(crate) const MAX_FRAMES: usize = 128;
pub(crate) const MAX_DURATION_MS: u32 = 10_000;
pub(crate) const MIN_FRAME_DURATION_MS: u32 = 8;

struct Chunk<'a> {
    kind: &'a [u8],
    data: &'a [u8],
}
fn le24(bytes: &[u8]) -> Option<u32> {
    Some(u32::from_le_bytes([
        *bytes.first()?,
        *bytes.get(1)?,
        *bytes.get(2)?,
        0,
    ]))
}
fn chunks(mut bytes: &[u8]) -> Option<Vec<Chunk<'_>>> {
    let mut result = Vec::new();
    while !bytes.is_empty() {
        if result.len() >= MAX_FRAMES + 8 {
            return None;
        }
        let head = bytes.get(..8)?;
        let size = usize::try_from(u32::from_le_bytes(head[4..8].try_into().ok()?)).ok()?;
        let end = 8usize.checked_add(size)?;
        let padded = end.checked_add(size & 1)?;
        if size & 1 != 0 && bytes.get(end) != Some(&0) {
            return None;
        }
        result.push(Chunk {
            kind: &head[..4],
            data: bytes.get(8..end)?,
        });
        bytes = bytes.get(padded..)?;
    }
    Some(result)
}

/// Return whether this is animated, after enforcing its corresponding size cap.
pub(crate) fn inspect(bytes: &[u8]) -> Option<bool> {
    if bytes.len() > MAX_ANIMATED_BYTES
        || bytes.get(..4)? != b"RIFF"
        || bytes.get(8..12)? != b"WEBP"
        || usize::try_from(u32::from_le_bytes(bytes.get(4..8)?.try_into().ok()?))
            .ok()?
            .checked_add(8)?
            != bytes.len()
    {
        return None;
    }
    let parts = chunks(bytes.get(12..)?)?;
    let first = parts.first()?;
    let animated = if first.kind == b"VP8X" {
        extended(&parts)?
    } else {
        let (width, height, _) = image(&parts)?;
        if width != SIDE || height != SIDE {
            return None;
        }
        false
    };
    if bytes.len()
        > if animated {
            MAX_ANIMATED_BYTES
        } else {
            MAX_STATIC_BYTES
        }
    {
        return None;
    }
    Some(animated)
}

fn extended(parts: &[Chunk<'_>]) -> Option<bool> {
    let header = parts.first()?.data;
    if header.len() != 10
        || header[0] & 0xc1 != 0
        || header[1..4] != [0; 3]
        || le24(&header[4..7])? + 1 != SIDE
        || le24(&header[7..10])? + 1 != SIDE
    {
        return None;
    }
    let flags = header[0];
    let animated = flags & 2 != 0;
    let mut index = 1;
    // ICC is forwarded as opaque bounded metadata, never parsed or inflated.
    if flags & 0x20 != 0 {
        if parts.get(index)?.kind != b"ICCP" || parts[index].data.is_empty() {
            return None;
        }
        index += 1;
    }
    let alpha = if animated {
        let control = parts.get(index)?;
        if control.kind != b"ANIM" || control.data.len() != 6 {
            return None;
        }
        index += 1;
        let mut count = 0;
        let mut duration = 0u32;
        let mut alpha = false;
        while parts.get(index).is_some_and(|part| part.kind == b"ANMF") {
            let frame = parts[index].data;
            let head = frame.get(..16)?;
            if head[15] & !3 != 0 {
                return None;
            }
            let x = le24(&head[..3])?.checked_mul(2)?;
            let y = le24(&head[3..6])?.checked_mul(2)?;
            let width = le24(&head[6..9])? + 1;
            let height = le24(&head[9..12])? + 1;
            let millis = le24(&head[12..15])?;
            duration = duration.checked_add(millis)?;
            count += 1;
            if count > MAX_FRAMES
                || millis < MIN_FRAME_DURATION_MS
                || duration > MAX_DURATION_MS
                || x.checked_add(width)? > SIDE
                || y.checked_add(height)? > SIDE
            {
                return None;
            }
            let frame_parts = chunks(frame.get(16..)?)?;
            let (image_width, image_height, image_alpha) = image(&frame_parts)?;
            if image_width != width || image_height != height {
                return None;
            }
            alpha |= image_alpha;
            index += 1;
        }
        if count < 2 {
            return None;
        }
        alpha
    } else {
        let end = index
            + if parts.get(index)?.kind == b"ALPH" {
                2
            } else {
                1
            };
        let (width, height, alpha) = image(parts.get(index..end)?)?;
        if width != SIDE || height != SIDE {
            return None;
        }
        index = end;
        alpha
    };
    if alpha != (flags & 0x10 != 0) {
        return None;
    }
    for (flag, kind) in [(8, &b"EXIF"[..]), (4, &b"XMP "[..])] {
        if flags & flag != 0 {
            let metadata = parts.get(index)?;
            if metadata.kind != kind || metadata.data.is_empty() {
                return None;
            }
            index += 1;
        }
    }
    // Refuse duplicate, unadvertised or unknown chunks rather than overlooking
    // a second bitstream/animation; metadata ordering is this bounded profile.
    (index == parts.len()).then_some(animated)
}

fn image(parts: &[Chunk<'_>]) -> Option<(u32, u32, bool)> {
    let first = parts.first()?;
    match first.kind {
        b"VP8L" if parts.len() == 1 => {
            let header = first.data.get(..5)?;
            if header[0] != 0x2f || header[4] >> 5 != 0 || first.data.len() <= 5 {
                return None;
            }
            let bits = u32::from_le_bytes(header[1..5].try_into().ok()?);
            Some((
                (bits & 0x3fff) + 1,
                ((bits >> 14) & 0x3fff) + 1,
                bits & (1 << 28) != 0,
            ))
        }
        b"VP8 " if parts.len() == 1 => {
            let (width, height) = vp8(first.data)?;
            Some((width, height, false))
        }
        b"ALPH" if parts.len() == 2 && parts[1].kind == b"VP8 " => {
            let (width, height) = vp8(parts[1].data)?;
            let header = *first.data.first()?;
            if header & 0xc0 != 0 || (header >> 4) & 3 > 1 || header & 3 > 1 {
                return None;
            }
            if (header & 3 == 0
                && first.data.len() != 1 + usize::try_from(width.checked_mul(height)?).ok()?)
                || (header & 3 == 1 && first.data.len() <= 1)
            {
                return None;
            }
            Some((width, height, true))
        }
        _ => None,
    }
}
fn vp8(bytes: &[u8]) -> Option<(u32, u32)> {
    let head = bytes.get(..10)?;
    let tag = le24(head)?;
    let partition = usize::try_from(tag >> 5).ok()?;
    if tag & 1 != 0
        || (tag >> 1) & 7 > 3
        || tag & 0x10 == 0
        || head[3..6] != [0x9d, 0x01, 0x2a]
        || partition == 0
        || partition.checked_add(10)? > bytes.len()
    {
        return None;
    }
    let width = u16::from_le_bytes(head[6..8].try_into().ok()?);
    let height = u16::from_le_bytes(head[8..10].try_into().ok()?);
    if width == 0 || height == 0 || width & 0xc000 != 0 || height & 0xc000 != 0 {
        return None;
    }
    Some((u32::from(width), u32::from(height)))
}

#[cfg(test)]
#[path = "outbound_sticker_tests.rs"]
pub(crate) mod tests;
