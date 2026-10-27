//! Borrowed, fixed-depth ISO BMFF traversal shared by native audio and video.
#[path = "bmff/aac.rs"]
pub(crate) mod aac;
#[path = "bmff/samples.rs"]
mod samples;
pub(crate) use samples::sample_ranges;

pub(crate) struct Atom<'a> {
    pub kind: [u8; 4],
    pub body: &'a [u8],
    pub body_offset: usize,
}
pub(crate) fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_be_bytes(
        bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}
pub(crate) fn atoms(bytes: &[u8], base: usize) -> Option<Vec<Atom<'_>>> {
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
pub(crate) fn one<'a, 'b>(atoms: &'b [Atom<'a>], kind: &[u8; 4]) -> Option<&'b Atom<'a>> {
    let mut found = atoms.iter().filter(|atom| &atom.kind == kind);
    let value = found.next()?;
    if found.next().is_some() {
        return None;
    }
    Some(value)
}
pub(crate) fn child<'a>(parent: &Atom<'a>) -> Option<Vec<Atom<'a>>> {
    atoms(parent.body, parent.body_offset)
}
