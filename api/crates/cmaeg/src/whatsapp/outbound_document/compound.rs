//! CFB allocation chains plus bounded Word FIB, BIFF8 and PowerPoint records.
use super::Document;
use std::io::{Cursor, Read};

fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}
fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}
pub(super) fn inspect(bytes: &[u8]) -> Option<Document> {
    let shift = u16_at(bytes, 30)?;
    if !matches!(shift, 9 | 12) || !bytes.len().is_multiple_of(1 << shift) {
        return None;
    }
    let sectors = bytes.len().checked_div(1 << shift)?.checked_sub(1)?;
    // Bound the reader's FAT allocation before opening attacker-controlled CFB.
    if u32_at(bytes, 44)? as usize > sectors || u32_at(bytes, 72)? as usize > sectors {
        return None;
    }
    let mut cfb = cfb::CompoundFile::open(Cursor::new(bytes)).ok()?;
    let entries: Vec<_> = cfb.walk().take(1025).collect();
    if entries.len() > 1024 {
        return None;
    }
    let mut total = 0u64;
    for entry in &entries {
        if entry.is_stream() {
            total = total.checked_add(entry.len())?;
            if entry.len() > 8 * 1024 * 1024 || total > 32 * 1024 * 1024 {
                return None;
            }
            // Check complete stream chains, including streams not used for classification.
            let read = std::io::copy(
                &mut cfb.open_stream(entry.path()).ok()?.take(entry.len() + 1),
                &mut std::io::sink(),
            )
            .ok()?;
            if read != entry.len() {
                return None;
            }
        }
    }
    let candidates: Vec<_> = ["WordDocument", "Workbook", "Book", "PowerPoint Document"]
        .into_iter()
        .filter(|name| cfb.is_stream(name))
        .collect();
    if candidates.len() != 1 || cfb.is_stream("EncryptedPackage") {
        return None;
    }
    let mut body = Vec::new();
    cfb.open_stream(candidates[0])
        .ok()?
        .read_to_end(&mut body)
        .ok()?;
    match candidates[0] {
        "WordDocument" => {
            let flags = u16_at(&body, 10)?;
            let table = if flags & 0x200 != 0 {
                "1Table"
            } else {
                "0Table"
            };
            (u16_at(&body, 0)? == 0xa5ec
                && matches!(
                    u16_at(&body, 2)?,
                    0x00c1 | 0x00d9 | 0x0101 | 0x010c | 0x0112
                )
                && flags & 0x8100 == 0
                && u32_at(&body, 24)? >= 32
                && u32_at(&body, 24)? <= u32_at(&body, 28)?
                && u32_at(&body, 28)? as usize <= body.len()
                && cfb
                    .entry(table)
                    .ok()
                    .is_some_and(|e| e.is_stream() && !e.is_empty()))
            .then_some(Document::Word)
        }
        "Workbook" | "Book" => excel(&body).then_some(Document::Excel),
        "PowerPoint Document" => {
            let mut user = Vec::new();
            cfb.open_stream("Current User")
                .ok()?
                .take(4097)
                .read_to_end(&mut user)
                .ok()?;
            (user.len() <= 4096
                && u16_at(&user, 2)? == 0x0ff6
                && u32_at(&user, 4)? >= 20
                && u32_at(&user, 4)? as usize <= user.len().checked_sub(8)?
                && u32_at(&user, 12)? == 0xe391c05f
                && powerpoint(&body))
            .then_some(Document::PowerPoint)
        }
        _ => None,
    }
}

fn excel(mut bytes: &[u8]) -> bool {
    let mut count = 0;
    let mut open = false;
    let mut globals = false;
    while !bytes.is_empty() && count < 65536 {
        // BIFF writers commonly pad their final stream to a 4096-byte boundary.
        if globals && !open && bytes.len() < 4096 && bytes.iter().all(|b| *b == 0) {
            return true;
        }
        let Some((kind, size)) = u16_at(bytes, 0).zip(u16_at(bytes, 2)) else {
            return false;
        };
        let size = usize::from(size);
        let Some(body) = bytes.get(4..4 + size) else {
            return false;
        };
        if size > 8224 || kind == 0x002f {
            return false;
        } // FILEPASS is encryption.
        if kind == 0x0809 {
            if open || size != 16 || u16_at(body, 0) != Some(0x0600) {
                return false;
            }
            if count == 0 {
                if u16_at(body, 2) != Some(0x0005) {
                    return false;
                }
                globals = true;
            }
            open = true;
        } else if !open {
            return false;
        } else if kind == 0x000a {
            if size != 0 {
                return false;
            }
            open = false;
        }
        bytes = &bytes[4 + size..];
        count += 1;
    }
    globals && !open && bytes.is_empty()
}

fn powerpoint(bytes: &[u8]) -> bool {
    let mut stack = vec![(bytes, 0usize)];
    let mut count = 0;
    let mut document = false;
    let mut atom = false;
    while let Some((mut bytes, depth)) = stack.pop() {
        while !bytes.is_empty() {
            if depth == 0 && document && bytes.len() < 4096 && bytes.iter().all(|b| *b == 0) {
                break;
            }
            count += 1;
            if count > 65536 || depth > 32 {
                return false;
            }
            let Some((version, kind)) = u16_at(bytes, 0).zip(u16_at(bytes, 2)) else {
                return false;
            };
            let Some(size) = u32_at(bytes, 4).map(|n| n as usize) else {
                return false;
            };
            let Some(body) = bytes.get(8..8 + size) else {
                return false;
            };
            if version & 15 == 15 {
                if kind == 1000 && depth == 0 {
                    document = true;
                }
                stack.push((body, depth + 1));
            } else if kind == 1001 && depth == 1 && size == 40 {
                atom = true;
            }
            bytes = &bytes[8 + size..];
        }
    }
    document && atom
}
