//! One complete Ogg logical stream with mono Opus, no chaining or multiplexing.
//! Pages are checked in place; only the two bounded header packets are copied.
pub(crate) fn mono_opus(bytes: &[u8]) -> bool {
    validate(bytes).is_some()
}
fn validate(mut bytes: &[u8]) -> Option<()> {
    let mut serial = None;
    let mut sequence = 0u32;
    let mut packet = Vec::new();
    let mut packet_size = 0usize;
    let mut packet_index = 0usize;
    let mut eos = false;
    while !bytes.is_empty() {
        let head = bytes.get(..27)?;
        if eos || head.get(..4)? != b"OggS" || head[4] != 0 || head[5] & !7 != 0 {
            return None;
        }
        let flags = head[5];
        let page_serial = u32::from_le_bytes(head[14..18].try_into().ok()?);
        let page_sequence = u32::from_le_bytes(head[18..22].try_into().ok()?);
        if serial.is_some_and(|value| value != page_serial)
            || page_sequence != sequence
            || (flags & 2 != 0) != (sequence == 0)
            || (flags & 1 != 0) != (packet_size > 0)
        {
            return None;
        }
        serial = Some(page_serial);
        sequence = sequence.checked_add(1)?;
        let laces = bytes.get(27..27 + usize::from(head[26]))?;
        if laces.is_empty() {
            return None;
        }
        let body_size: usize = laces.iter().map(|byte| usize::from(*byte)).sum();
        let page_size = 27 + laces.len() + body_size;
        let page = bytes.get(..page_size)?;
        let checksum = u32::from_le_bytes(head[22..26].try_into().ok()?);
        if crc(page) != checksum {
            return None;
        }
        let mut body = page.get(27 + laces.len()..)?;
        for &lace in laces {
            let size = usize::from(lace);
            let data = body.get(..size)?;
            body = body.get(size..)?;
            packet_size = packet_size.checked_add(size)?;
            if packet_index < 2 {
                if packet_size > if packet_index == 0 { 19 } else { 65536 } {
                    return None;
                }
                packet.extend_from_slice(data);
            }
            if lace < 255 {
                match packet_index {
                    0 if packet.len() != 19
                        || packet.get(..8)? != b"OpusHead"
                        || packet[8] != 1
                        || packet[9] != 1
                        || packet[18] != 0 =>
                    {
                        return None
                    }
                    1 if !tags(&packet) => return None,
                    index if index >= 2 && packet_size == 0 => return None,
                    _ => {}
                }
                packet_index += 1;
                packet_size = 0;
                packet.clear();
            }
        }
        eos = flags & 4 != 0;
        if eos && packet_size > 0 {
            return None;
        }
        bytes = bytes.get(page_size..)?;
    }
    (eos && packet_size == 0 && packet_index >= 3).then_some(())
}
fn tags(bytes: &[u8]) -> bool {
    fn check(bytes: &[u8]) -> Option<()> {
        if bytes.get(..8)? != b"OpusTags" {
            return None;
        }
        let vendor = u32::from_le_bytes(bytes.get(8..12)?.try_into().ok()?) as usize;
        let mut offset = 12usize.checked_add(vendor)?;
        let comments =
            u32::from_le_bytes(bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?);
        if comments > 1024 {
            return None;
        }
        offset += 4;
        for _ in 0..comments {
            let length =
                u32::from_le_bytes(bytes.get(offset..offset.checked_add(4)?)?.try_into().ok()?)
                    as usize;
            offset = offset.checked_add(4)?.checked_add(length)?;
            bytes.get(..offset)?;
        }
        // OpusTags may end in padding; its outer packet is independently capped.
        Some(())
    }
    check(bytes).is_some()
}

/// Ogg's non-reflected polynomial; the checksum field is treated as zero.
fn crc(page: &[u8]) -> u32 {
    let mut result = 0u32;
    for (index, &byte) in page.iter().enumerate() {
        let byte = if (22..26).contains(&index) { 0 } else { byte };
        result ^= u32::from(byte) << 24;
        for _ in 0..8 {
            result = if result & 0x8000_0000 != 0 {
                (result << 1) ^ 0x04c1_1db7
            } else {
                result << 1
            };
        }
    }
    result
}

#[cfg(test)]
pub(crate) fn repair_checksums(mut bytes: &mut [u8]) {
    while !bytes.is_empty() {
        let count = usize::from(bytes[26]);
        let size = 27
            + count
            + bytes[27..27 + count]
                .iter()
                .map(|v| usize::from(*v))
                .sum::<usize>();
        let checksum = crc(&bytes[..size]);
        bytes[22..26].copy_from_slice(&checksum.to_le_bytes());
        bytes = &mut bytes[size..];
    }
}
