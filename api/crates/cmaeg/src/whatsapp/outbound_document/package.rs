//! Complete CRC-checked ZIP framing and OOXML package identity, without extraction.
use super::{xml, Document};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Cursor, Read},
};
use zip::{CompressionMethod, ZipArchive};

const TYPES: &str = "http://schemas.openxmlformats.org/package/2006/content-types";
const RELS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
const MAIN_REL: [&str; 2] = [
    "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument",
    "http://purl.oclc.org/ooxml/officeDocument/relationships/officeDocument",
];
const MAIN: [(&str, &str, &str, [&str; 2], Document); 3] = [
    (
        "word/document.xml",
        "application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml",
        "document",
        [
            "http://schemas.openxmlformats.org/wordprocessingml/2006/main",
            "http://purl.oclc.org/ooxml/wordprocessingml/main",
        ],
        Document::WordXml,
    ),
    (
        "xl/workbook.xml",
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml",
        "workbook",
        [
            "http://schemas.openxmlformats.org/spreadsheetml/2006/main",
            "http://purl.oclc.org/ooxml/spreadsheetml/main",
        ],
        Document::ExcelXml,
    ),
    (
        "ppt/presentation.xml",
        "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml",
        "presentation",
        [
            "http://schemas.openxmlformats.org/presentationml/2006/main",
            "http://purl.oclc.org/ooxml/presentationml/main",
        ],
        Document::PowerPointXml,
    ),
];

fn u16_at(bytes: &[u8], offset: usize) -> Option<usize> {
    Some(u16::from_le_bytes(bytes.get(offset..offset + 2)?.try_into().ok()?) as usize)
}
fn u32_at(bytes: &[u8], offset: usize) -> Option<usize> {
    Some(u32::from_le_bytes(bytes.get(offset..offset + 4)?.try_into().ok()?) as usize)
}
// Refuse ZIP64/multidisk and cap the directory before the ZIP reader allocates it.
fn directory(bytes: &[u8], minimum_entries: usize) -> Option<(usize, usize)> {
    let end = (bytes.len().saturating_sub(65557)..bytes.len().checked_sub(21)?)
        .rev()
        .find(|&i| {
            bytes.get(i..i + 4) == Some(b"PK\x05\x06")
                && u16_at(bytes, i + 20).is_some_and(|n| i + 22 + n == bytes.len())
        })?;
    let count = u16_at(bytes, end + 10)?;
    let start = u32_at(bytes, end + 16)?;
    if end.checked_sub(20).and_then(|at| bytes.get(at..at + 4)) == Some(b"PK\x06\x07")
        || u16_at(bytes, end + 4)? != 0
        || u16_at(bytes, end + 6)? != 0
        || u16_at(bytes, end + 8)? != count
        || !(minimum_entries..=1024).contains(&count)
        || (count == 0 && (start != 0 || end != 0))
        || (count > 0 && !bytes.starts_with(b"PK\x03\x04"))
        || start.checked_add(u32_at(bytes, end + 12)?)? != end
    {
        return None;
    }
    let mut at = start;
    let mut names = BTreeSet::new();
    for _ in 0..count {
        if bytes.get(at..at + 4) != Some(b"PK\x01\x02") {
            return None;
        }
        let name_len = u16_at(bytes, at + 28)?;
        if name_len == 0 || name_len > 256 {
            return None;
        }
        let name = std::str::from_utf8(bytes.get(at + 46..at + 46 + name_len)?).ok()?;
        if name.starts_with('/')
            || name.contains(['\\', '\0'])
            || name
                .trim_end_matches('/')
                .split('/')
                .any(|p| p.is_empty() || matches!(p, "." | ".."))
            || !names.insert(name.to_ascii_lowercase())
        {
            return None;
        }
        let local = u32_at(bytes, at + 42)?;
        if u16_at(bytes, at + 34)? != 0
            || [20, 24, 42]
                .into_iter()
                .any(|offset| u32_at(bytes, at + offset) == Some(u32::MAX as usize))
            || bytes.get(local..local + 4) != Some(b"PK\x03\x04")
            || u16_at(bytes, local + 6)? != u16_at(bytes, at + 8)?
            || u16_at(bytes, local + 8)? != u16_at(bytes, at + 10)?
            || u16_at(bytes, local + 26)? != name_len
            || bytes.get(local + 30..local + 30 + name_len)? != name.as_bytes()
        {
            return None;
        }
        let extra_start = at + 46 + name_len;
        let extra_end = extra_start.checked_add(u16_at(bytes, at + 30)?)?;
        let mut extra = extra_start;
        while extra < extra_end {
            if u16_at(bytes, extra)? == 1 {
                return None;
            } // ZIP64 extra field.
            extra = extra.checked_add(4 + u16_at(bytes, extra + 2)?)?;
        }
        if extra != extra_end {
            return None;
        }
        at = at.checked_add(46 + name_len + u16_at(bytes, at + 30)? + u16_at(bytes, at + 32)?)?;
        if at > end {
            return None;
        }
    }
    (at == end).then_some((count, start))
}

fn contents(
    bytes: &[u8],
    minimum_entries: usize,
    retain_documents: bool,
) -> Option<BTreeMap<String, Vec<u8>>> {
    let (count, central_start) = directory(bytes, minimum_entries)?;
    let mut archive = ZipArchive::new(Cursor::new(bytes)).ok()?;
    if archive.len() != count || archive.offset() != 0 || archive.has_overlapping_entries().ok()? {
        return None;
    }
    let mut remaining = 32 * 1024 * 1024u64;
    let mut retained = BTreeMap::new();
    for index in 0..count {
        let mut file = archive.by_index(index).ok()?;
        let name = file.name().ok()?.into_owned();
        let size = file.size();
        if file.encrypted()
            || !matches!(
                file.compression(),
                CompressionMethod::Stored | CompressionMethod::Deflated
            )
            || size > 8 * 1024 * 1024
            || size > remaining
            || file.data_start()?.checked_add(file.compressed_size())? > central_start as u64
        {
            return None;
        }
        remaining -= size;
        let wanted = retain_documents
            && (matches!(name.as_str(), "[Content_Types].xml" | "_rels/.rels")
                || MAIN.iter().any(|m| m.0 == name));
        // Read even unselected entries through EOF so truncated streams and CRC errors fail.
        if wanted {
            let mut body = Vec::new();
            file.by_ref().take(size + 1).read_to_end(&mut body).ok()?;
            if body.len() as u64 != size {
                return None;
            }
            retained.insert(name, body);
        } else if std::io::copy(&mut file.take(size + 1), &mut std::io::sink()).ok()? != size {
            return None;
        }
    }
    Some(retained)
}

/// Validate a bounded complete archive without extracting or retaining its contents.
/// This does not add archive support to WhatsApp's document profile.
pub(super) fn archive(bytes: &[u8]) -> bool {
    contents(bytes, 0, false).is_some()
}

pub(super) fn inspect(bytes: &[u8]) -> Option<Document> {
    let retained = contents(bytes, 3, true)?;
    let mut selected = None;
    let mut part_names = BTreeSet::new();
    xml::parse(
        retained.get("[Content_Types].xml")?,
        "Types",
        &[TYPES],
        |depth, ns, name, attrs| {
            if depth == 1 && ns == TYPES && name == "Override" {
                let part = attrs.get("PartName")?;
                if !part_names.insert(part.clone()) {
                    return None;
                }
                let mime = attrs.get("ContentType")?;
                if let Some(index) = MAIN
                    .iter()
                    .position(|m| part.strip_prefix('/') == Some(m.0))
                {
                    if mime != MAIN[index].1 || selected.replace(index).is_some() {
                        return None;
                    }
                }
            }
            Some(())
        },
    )?;
    let selected = selected?;
    let (path, _, root, namespaces, document) = MAIN[selected];
    let mut main_relationship = false;
    xml::parse(
        retained.get("_rels/.rels")?,
        "Relationships",
        &[RELS],
        |depth, ns, name, attrs| {
            if depth == 1
                && ns == RELS
                && name == "Relationship"
                && MAIN_REL.contains(&attrs.get("Type")?.as_str())
            {
                let target = attrs.get("Target")?;
                if main_relationship
                    || target.strip_prefix('/').unwrap_or(target) != path
                    || attrs
                        .get("TargetMode")
                        .is_some_and(|mode| mode != "Internal")
                {
                    return None;
                }
                main_relationship = true;
            }
            Some(())
        },
    )?;
    if !main_relationship {
        return None;
    }
    xml::parse(
        retained.get(path)?,
        root,
        &namespaces,
        |_, _, _, _| Some(()),
    )?;
    Some(document)
}
