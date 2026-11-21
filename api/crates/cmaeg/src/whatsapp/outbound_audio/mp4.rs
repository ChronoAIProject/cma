//! Non-fragmented M4A with one self-contained, unencrypted AAC-LC audio track.
//! All traversal is borrowed, depth is fixed and each container has a box cap.

use super::super::bmff::{aac, atoms, child, one, sample_ranges, u32_at};

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
    aac::sample_entry(one(&descriptions, b"mp4a")?)?;
    sample_ranges(&top, &table, 262_144)?;
    Some(())
}
