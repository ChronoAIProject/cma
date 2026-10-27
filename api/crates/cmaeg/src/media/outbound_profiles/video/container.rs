//! Track structure and non-reordered media timing for the admitted BMFF subset.
use super::super::bmff::{atoms, child, one, u32_at, Atom};

pub(super) struct Track<'a> {
    pub kind: [u8; 4],
    pub id: u32,
    pub width: u32,
    pub height: u32,
    pub table: Vec<Atom<'a>>,
    pub descriptions: Vec<Atom<'a>>,
}
pub(super) fn only(atoms: &[Atom<'_>], kinds: &[[u8; 4]]) -> Option<()> {
    atoms
        .iter()
        .all(|atom| kinds.contains(&atom.kind))
        .then_some(())
}
fn clock(bytes: &[u8]) -> Option<(u32, u64)> {
    if bytes.get(1..4)? != [0, 0, 0] {
        return None;
    }
    let (scale, duration) = match bytes.first()? {
        0 => (u32_at(bytes, 12)?, u64::from(u32_at(bytes, 16)?)),
        1 => (
            u32_at(bytes, 20)?,
            u64::from_be_bytes(bytes.get(24..32)?.try_into().ok()?),
        ),
        _ => return None,
    };
    (scale != 0
        && duration != 0
        && duration <= u64::from(scale) * u64::from(super::MAX_DURATION_MS) / 1000)
        .then_some((scale, duration))
}
pub(super) fn movie(header: &Atom<'_>) -> Option<(u32, u64)> {
    let timing = clock(header.body)?;
    let length = match header.body.first()? {
        0 => 100,
        1 => 112,
        _ => return None,
    };
    (header.body.len() == length && u32_at(header.body, length - 80)? == 65536).then_some(timing)
}
pub(super) fn track<'a>(atom: &Atom<'a>, movie: (u32, u64)) -> Option<Track<'a>> {
    let parts = child(atom)?;
    only(&parts, &[*b"tkhd", *b"mdia", *b"edts"])?;
    let header = one(&parts, b"tkhd")?.body;
    if header.get(1..3)? != [0, 0] || header.get(3)? & !7 != 0 {
        return None;
    }
    let offset = match header.first()? {
        0 if header.len() == 84 => 0,
        1 if header.len() == 96 => 12,
        _ => return None,
    };
    let id = u32_at(header, 12 + if offset == 0 { 0 } else { 8 })?;
    let duration = if offset == 0 {
        u64::from(u32_at(header, 20)?)
    } else {
        u64::from_be_bytes(header.get(28..36)?.try_into().ok()?)
    };
    if id == 0 || duration == 0 || duration > movie.1 {
        return None;
    }
    let matrix = header.get(40 + offset..76 + offset)?;
    for (index, expected) in [65536, 0, 0, 0, 65536, 0, 0, 0, 1 << 30]
        .into_iter()
        .enumerate()
    {
        if u32_at(matrix, index * 4)? != expected {
            return None;
        }
    }
    let width = u32_at(header, 76 + offset)?;
    let height = u32_at(header, 80 + offset)?;
    if width & 65535 != 0 || height & 65535 != 0 {
        return None;
    }
    let mut edit_ranges = Vec::new();
    if parts.iter().any(|part| part.kind == *b"edts") {
        let edits = child(one(&parts, b"edts")?)?;
        only(&edits, &[*b"elst"])?;
        let edits = one(&edits, b"elst")?.body;
        let count = u32_at(edits, 4)? as usize;
        if edits.get(..4)? != [0, 0, 0, 0]
            || !(1..=2).contains(&count)
            || edits.len() != 8 + count * 12
        {
            return None;
        }
        for entry in edits[8..].chunks_exact(12) {
            let start = i32::from_be_bytes(entry[4..8].try_into().ok()?);
            let length = u64::from(u32_at(entry, 0)?);
            if length == 0
                || start < -1
                || entry[8..12] != [0, 1, 0, 0]
                || (start == -1 && !edit_ranges.is_empty())
            {
                return None;
            }
            edit_ranges.push((length, start));
        }
    }
    let media = child(one(&parts, b"mdia")?)?;
    only(&media, &[*b"mdhd", *b"hdlr", *b"minf"])?;
    let handler = one(&media, b"hdlr")?.body;
    if handler.get(..4)? != [0, 0, 0, 0] {
        return None;
    }
    let kind = handler.get(8..12)?.try_into().ok()?;
    if kind != *b"vide" && kind != *b"soun" {
        return None;
    }
    let info = child(one(&media, b"minf")?)?;
    only(&info, &[*b"vmhd", *b"smhd", *b"dinf", *b"stbl"])?;
    if info
        .iter()
        .any(|entry| entry.kind == if kind == *b"vide" { *b"smhd" } else { *b"vmhd" })
    {
        return None;
    }
    let media_header = one(&info, if kind == *b"vide" { b"vmhd" } else { b"smhd" })?.body;
    if (kind == *b"vide" && (media_header.len() != 12 || media_header.get(..4)? != [0, 0, 0, 1]))
        || (kind == *b"soun" && (media_header.len() != 8 || media_header.get(..4)? != [0, 0, 0, 0]))
    {
        return None;
    }
    let references = child(one(&info, b"dinf")?)?;
    only(&references, &[*b"dref"])?;
    let reference = one(&references, b"dref")?;
    if reference.body.get(..8)? != [0, 0, 0, 0, 0, 0, 0, 1] {
        return None;
    }
    let urls = atoms(reference.body.get(8..)?, reference.body_offset + 8)?;
    if urls.len() != 1 || urls[0].kind != *b"url " || urls[0].body != [0, 0, 0, 1] {
        return None;
    }
    let table = child(one(&info, b"stbl")?)?;
    only(
        &table,
        &[
            *b"stsd", *b"stts", *b"stss", *b"ctts", *b"stsc", *b"stsz", *b"stco", *b"co64",
            *b"sgpd", *b"sbgp",
        ],
    )?;
    // AAC priming roll groups do not alter sample byte ranges; no video groups.
    if kind == *b"vide"
        && table
            .iter()
            .any(|entry| matches!(&entry.kind, b"sgpd" | b"sbgp"))
    {
        return None;
    }
    let description = one(&table, b"stsd")?;
    if description.body.get(..8)? != [0, 0, 0, 0, 0, 0, 0, 1] {
        return None;
    }
    let descriptions = atoms(description.body.get(8..)?, description.body_offset + 8)?;
    if descriptions.len() != 1 {
        return None;
    }
    let count = u32_at(one(&table, b"stsz")?.body, 8)?;
    let mdhd = one(&media, b"mdhd")?.body;
    if !matches!((mdhd.first()?, mdhd.len()), (0, 24) | (1, 36)) {
        return None;
    }
    let (scale, media_duration) = clock(mdhd)?;
    if !edit_ranges.is_empty() {
        let total: u64 = edit_ranges.iter().map(|(length, _)| length).sum();
        if total != duration || edit_ranges.iter().all(|(_, start)| *start < 0) {
            return None;
        }
        for (length, start) in edit_ranges {
            if start >= 0
                && u64::try_from(start)
                    .ok()?
                    .checked_mul(u64::from(movie.0))?
                    .checked_add(length.checked_mul(u64::from(scale))?)?
                    > media_duration
                        .checked_mul(u64::from(movie.0))?
                        .checked_add(u64::from(scale))?
            {
                return None;
            }
        }
    } else {
        let track_ticks = duration.checked_mul(u64::from(scale))?;
        let media_ticks = media_duration.checked_mul(u64::from(movie.0))?;
        if track_ticks.abs_diff(media_ticks) > u64::from(scale) {
            return None;
        }
    }
    timing(&table, count, scale, media_duration, kind == *b"vide")?;
    audio_groups(&table, count)?;
    Some(Track {
        kind,
        id,
        width: width >> 16,
        height: height >> 16,
        table,
        descriptions,
    })
}
fn entries<'a>(table: &'a [Atom<'a>], kind: &[u8; 4]) -> Option<&'a [u8]> {
    let bytes = one(table, kind)?.body;
    let count = usize::try_from(u32_at(bytes, 4)?).ok()?;
    if bytes.get(..4)? != [0, 0, 0, 0]
        || count == 0
        || count > super::MAX_SAMPLES
        || bytes.len() != 8 + count * 8
    {
        return None;
    }
    bytes.get(8..)
}
fn timing(table: &[Atom<'_>], count: u32, scale: u32, duration: u64, video: bool) -> Option<()> {
    let mut samples = 0u64;
    let mut ticks = 0u64;
    for entry in entries(table, b"stts")?.chunks_exact(8) {
        let n = u64::from(u32_at(entry, 0)?);
        let delta = u64::from(u32_at(entry, 4)?);
        if n == 0
            || delta == 0
            || (video && delta * u64::from(super::MAX_FRAME_RATE) < u64::from(scale))
        {
            return None;
        }
        samples = samples.checked_add(n)?;
        ticks = ticks.checked_add(n.checked_mul(delta)?)?;
    }
    if samples != u64::from(count) || ticks != duration {
        return None;
    }
    if table.iter().any(|entry| entry.kind == *b"ctts") {
        let mut samples = 0u64;
        for entry in entries(table, b"ctts")?.chunks_exact(8) {
            let n = u64::from(u32_at(entry, 0)?);
            if n == 0 || u32_at(entry, 4)? != 0 {
                return None;
            }
            samples = samples.checked_add(n)?;
        }
        if samples != u64::from(count) {
            return None;
        }
    }
    Some(())
}
pub(super) fn sync_samples(table: &[Atom<'_>], idr: &[bool]) -> Option<()> {
    if !table.iter().any(|entry| entry.kind == *b"stss") {
        return idr.iter().all(|idr| *idr).then_some(());
    }
    let bytes = one(table, b"stss")?.body;
    let count = usize::try_from(u32_at(bytes, 4)?).ok()?;
    if bytes.get(..4)? != [0, 0, 0, 0]
        || bytes.len() != 8 + count.checked_mul(4)?
        || count != idr.iter().filter(|idr| **idr).count()
    {
        return None;
    }
    let mut listed = bytes[8..].chunks_exact(4);
    for (index, idr) in idr.iter().enumerate() {
        if *idr && u32_at(listed.next()?, 0)? as usize != index + 1 {
            return None;
        }
    }
    Some(())
}

// Only the demonstrated AAC priming roll group is permitted; encryption/sample
// replacement group descriptions must not hide behind an otherwise plain mp4a.
fn audio_groups(table: &[Atom<'_>], count: u32) -> Option<()> {
    let descriptions = table.iter().any(|atom| atom.kind == *b"sgpd");
    let mapping = table.iter().any(|atom| atom.kind == *b"sbgp");
    if !descriptions && !mapping {
        return Some(());
    }
    if !descriptions || !mapping {
        return None;
    }
    let group = one(table, b"sgpd")?.body;
    if group
        != [
            1, 0, 0, 0, b'r', b'o', b'l', b'l', 0, 0, 0, 2, 0, 0, 0, 1, 255, 255,
        ]
    {
        return None;
    }
    let map = one(table, b"sbgp")?.body;
    if map.len() != 20
        || map[..12] != [0, 0, 0, 0, b'r', b'o', b'l', b'l', 0, 0, 0, 1]
        || u32_at(map, 12)? != count
        || u32_at(map, 16)? != 1
    {
        return None;
    }
    Some(())
}
