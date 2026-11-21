//! Checked sample/chunk byte ranges; callers decide overlap and timing policy.
use super::{one, u32_at, Atom};
use std::ops::Range;
pub(crate) fn sample_ranges(
    top: &[Atom<'_>],
    table: &[Atom<'_>],
    limit: usize,
) -> Option<Vec<Range<usize>>> {
    let size = one(table, b"stsz")?.body;
    if size.get(..4)? != [0, 0, 0, 0] {
        return None;
    }
    let fixed = u32_at(size, 4)?;
    let sample_count = usize::try_from(u32_at(size, 8)?).ok()?;
    if sample_count == 0 || sample_count > limit {
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
    let mapping = one(table, b"stsc")?.body;
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
    let mut ranges = Vec::with_capacity(sample_count);
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

        let raw = offsets
            .body
            .get(8 + index * width..8 + (index + 1) * width)?;
        let offset = if wide {
            u64::from_be_bytes(raw.try_into().ok()?)
        } else {
            u64::from(u32::from_be_bytes(raw.try_into().ok()?))
        };
        let mut position = usize::try_from(offset).ok()?;
        for sample_index in sample..sample_end {
            let length = if fixed == 0 {
                u32_at(size, 12 + sample_index * 4)?
            } else {
                fixed
            };
            let end = position.checked_add(usize::try_from(length).ok()?)?;
            ranges.push(position..end);
            position = end;
        }
        sample = sample_end;
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
    (sample == sample_count).then_some(ranges)
}
