//! Non-fragmented M4A with one self-contained, unencrypted AAC-LC audio track.
//! All traversal is borrowed, depth is fixed and each container has a box cap.

struct Atom<'a> {
    kind: [u8; 4],
    body: &'a [u8],
    body_offset: usize,
}
fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}
fn atoms(bytes: &[u8], base: usize) -> Option<Vec<Atom<'_>>> {
    let mut result = Vec::new();
    let mut offset = 0usize;
    while offset < bytes.len() {
        if result.len() >= 256 {
            return None;
        }
        let size = u32_at(bytes, offset)?;
        let kind: [u8; 4] = bytes.get(offset + 4..offset + 8)?.try_into().ok()?;
        let (size, header) = match size {
            0 if kind == *b"mdat" => (bytes.len() - offset, 8),
            0 => return None,
            1 => (
                usize::try_from(u64::from_be_bytes(
                    bytes.get(offset + 8..offset + 16)?.try_into().ok()?,
                ))
                .ok()?,
                16,
            ),
            size => (usize::try_from(size).ok()?, 8),
        };
        if size < header {
            return None;
        }
        let end = offset.checked_add(size)?;
        result.push(Atom {
            kind,
            body: bytes.get(offset + header..end)?,
            body_offset: base.checked_add(offset + header)?,
        });
        offset = end;
    }
    Some(result)
}
fn one<'a>(atoms: &'a [Atom<'a>], kind: &[u8; 4]) -> Option<&'a Atom<'a>> {
    let mut found = atoms.iter().filter(|atom| &atom.kind == kind);
    let value = found.next()?;
    if found.next().is_some() {
        return None;
    }
    Some(value)
}
fn child<'a>(parent: &Atom<'a>) -> Option<Vec<Atom<'a>>> {
    atoms(parent.body, parent.body_offset)
}

pub(super) fn aac_audio(bytes: &[u8]) -> bool {
    validate(bytes).is_some()
}
fn validate(bytes: &[u8]) -> Option<()> {
    let top = atoms(bytes, 0)?;
    if top
        .iter()
        .any(|atom| matches!(&atom.kind, b"moof" | b"pssh"))
    {
        return None;
    }
    let brand = one(&top, b"ftyp")?.body;
    if brand.len() < 8
        || brand.len() % 4 != 0
        || !matches!(
            brand.get(..4)?,
            b"M4A " | b"isom" | b"mp41" | b"mp42" | b"iso2"
        )
    {
        return None;
    }
    let moov = child(one(&top, b"moov")?)?;
    if moov.iter().any(|atom| &atom.kind == b"mvex") {
        return None;
    }
    let track = child(one(&moov, b"trak")?)?;
    let media = child(one(&track, b"mdia")?)?;
    let handler = one(&media, b"hdlr")?.body;
    if handler.get(..4)? != [0, 0, 0, 0] || handler.get(8..12)? != b"soun" {
        return None;
    }
    let info = child(one(&media, b"minf")?)?;
    let dinf = child(one(&info, b"dinf")?)?;
    let dref = one(&dinf, b"dref")?;
    if dref.body.get(..4)? != [0, 0, 0, 0] || u32_at(dref.body, 4)? != 1 {
        return None;
    }
    let refs = atoms(dref.body.get(8..)?, dref.body_offset + 8)?;
    if refs.len() != 1 || refs[0].kind != *b"url " || refs[0].body != [0, 0, 0, 1] {
        // No external file/URL reference may be uploaded as a self-contained track.
        return None;
    }
    let table = child(one(&info, b"stbl")?)?;
    if table
        .iter()
        .any(|atom| matches!(&atom.kind, b"senc" | b"saio" | b"saiz"))
    {
        return None;
    }
    let stsd = one(&table, b"stsd")?;
    if stsd.body.get(..4)? != [0, 0, 0, 0] || u32_at(stsd.body, 4)? != 1 {
        return None;
    }
    let descriptions = atoms(stsd.body.get(8..)?, stsd.body_offset + 8)?;
    if descriptions.len() != 1 {
        return None;
    }
    let entry = one(&descriptions, b"mp4a")?;
    let header = entry.body.get(..28)?;
    if header[..6] != [0; 6]
        || header[6..8] != [0, 1]
        || header[8..16] != [0; 8]
        || !matches!(u16::from_be_bytes([header[16], header[17]]), 1..=8)
        || header[18..20] != [0, 16]
        || u32_at(header, 24)? == 0
    {
        return None;
    }
    let extra = atoms(entry.body.get(28..)?, entry.body_offset + 28)?;
    if extra.iter().any(|atom| &atom.kind == b"sinf") {
        return None;
    }
    let (rate, channels) = aac_descriptor(one(&extra, b"esds")?.body)?;
    if u16::from_be_bytes([header[16], header[17]]) != channels
        || u32_at(header, 24)? != rate.checked_mul(65536)?
    {
        return None;
    }
    let size = one(&table, b"stsz")?.body;
    if size.get(..4)? != [0, 0, 0, 0] {
        return None;
    }
    let fixed = u32_at(size, 4)?;
    let sample_count = usize::try_from(u32_at(size, 8)?).ok()?;
    if sample_count == 0 || sample_count > 262_144 {
        return None;
    }
    let total = if fixed == 0 {
        if size.len() != 12 + sample_count * 4 {
            return None;
        }
        let mut total = 0u64;
        for index in 0..sample_count {
            let length = u32_at(size, 12 + index * 4)?;
            if length == 0 {
                return None;
            }
            total += u64::from(length);
        }
        total
    } else {
        if size.len() != 12 {
            return None;
        }
        u64::from(fixed) * sample_count as u64
    };
    let media_bytes: usize = top
        .iter()
        .filter(|atom| &atom.kind == b"mdat")
        .map(|atom| atom.body.len())
        .sum();
    if total > media_bytes as u64 {
        return None;
    }
    let offsets = table
        .iter()
        .filter(|atom| matches!(&atom.kind, b"stco" | b"co64"))
        .collect::<Vec<_>>();
    if offsets.len() != 1 {
        return None;
    }
    let offsets = offsets[0];
    let wide = offsets.kind == *b"co64";
    let count = usize::try_from(u32_at(offsets.body, 4)?).ok()?;
    let width = if wide { 8 } else { 4 };
    if offsets.body.get(..4)? != [0, 0, 0, 0]
        || count == 0
        || count > 262_144
        || offsets.body.len() != 8 + count * width
    {
        return None;
    }
    let mapping = one(&table, b"stsc")?.body;
    let entries = usize::try_from(u32_at(mapping, 4)?).ok()?;
    if mapping.get(..4)? != [0, 0, 0, 0]
        || entries == 0
        || entries > count
        || mapping.len() != 8 + entries * 12
    {
        return None;
    }
    let mut previous = 0;
    for index in 0..entries {
        let first = usize::try_from(u32_at(mapping, 8 + index * 12)?).ok()?;
        if first <= previous
            || first > count
            || (index == 0 && first != 1)
            || u32_at(mapping, 12 + index * 12)? == 0
            || u32_at(mapping, 16 + index * 12)? != 1
        {
            return None;
        }
        previous = first;
    }
    let mut mapping_index = 0;
    let mut sample = 0usize;
    for index in 0..count {
        if mapping_index + 1 < entries
            && usize::try_from(u32_at(mapping, 8 + (mapping_index + 1) * 12)?).ok()? == index + 1
        {
            mapping_index += 1;
        }
        let chunk_samples = usize::try_from(u32_at(mapping, 12 + mapping_index * 12)?).ok()?;
        let sample_end = sample.checked_add(chunk_samples)?;
        if sample_end > sample_count {
            return None;
        }
        let mut chunk_bytes = 0u64;
        for sample_index in sample..sample_end {
            chunk_bytes += u64::from(if fixed == 0 {
                u32_at(size, 12 + sample_index * 4)?
            } else {
                fixed
            });
        }
        sample = sample_end;
        let raw = offsets
            .body
            .get(8 + index * width..8 + (index + 1) * width)?;
        let offset = if wide {
            u64::from_be_bytes(raw.try_into().ok()?)
        } else {
            u64::from(u32::from_be_bytes(raw.try_into().ok()?))
        };
        if !top.iter().any(|atom| {
            atom.kind == *b"mdat"
                && offset >= atom.body_offset as u64
                && offset
                    .checked_add(chunk_bytes)
                    .is_some_and(|end| end <= (atom.body_offset + atom.body.len()) as u64)
        }) {
            return None;
        }
    }
    (sample == sample_count).then_some(())
}

/// ISO descriptor lengths have at most four continuation bytes.
fn descriptor(bytes: &[u8], tag: u8) -> Option<(&[u8], &[u8])> {
    if bytes.first() != Some(&tag) {
        return None;
    }
    let mut length = 0usize;
    for index in 1..=4 {
        let byte = *bytes.get(index)?;
        length = length
            .checked_mul(128)?
            .checked_add(usize::from(byte & 127))?;
        if byte & 128 == 0 {
            let end = (index + 1).checked_add(length)?;
            return Some((bytes.get(index + 1..end)?, bytes.get(end..)?));
        }
    }
    None
}
fn aac_descriptor(bytes: &[u8]) -> Option<(u32, u16)> {
    if bytes.get(..4)? != [0, 0, 0, 0] {
        return None;
    }
    let (es, rest) = descriptor(bytes.get(4..)?, 3)?;
    if !rest.is_empty() || es.get(2)? & 0xe0 != 0 {
        return None;
    }
    let (decoder, _) = descriptor(es.get(3..)?, 4)?;
    // MPEG-4 Audio with one AudioSpecificConfig, AAC-LC, indexed sample rate,
    // and a channel configuration (no program-config-element interpretation).
    if decoder.first() != Some(&0x40) || decoder.get(1)? & 0xfc != 0x14 {
        return None;
    }
    let (config, rest) = descriptor(decoder.get(13..)?, 5)?;
    if !rest.is_empty() || config.len() < 2 || config.len() > 64 {
        return None;
    }
    let object = config[0] >> 3;
    let frequency = ((config[0] & 7) << 1) | (config[1] >> 7);
    let channels = (config[1] >> 3) & 15;
    if object != 2 || frequency > 12 || !(1..=7).contains(&channels) || config[1] & 7 != 0 {
        return None;
    }
    // Plain AAC-LC ASC, or the common explicit "SBR absent" extension.
    // Other GASpecificConfig/extensions require a separately demonstrated profile.
    if config[2..].iter().any(|byte| *byte != 0)
        && !(config.get(2..5) == Some(&[0x56, 0xe5, 0])
            && config[5..].iter().all(|byte| *byte == 0))
    {
        return None;
    }
    let rate = [
        96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350,
    ][usize::from(frequency)];
    Some((
        rate,
        if channels == 7 {
            8
        } else {
            u16::from(channels)
        },
    ))
}
