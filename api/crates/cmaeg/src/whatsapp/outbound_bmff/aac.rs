//! AAC-LC sample entry and AudioSpecificConfig validation.
use super::{atoms, one, u32_at, Atom};
pub(in crate::whatsapp::outbound) fn sample_entry(entry: &Atom<'_>) -> Option<()> {
    if entry.kind != *b"mp4a" {
        return None;
    }
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
    Some(())
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
