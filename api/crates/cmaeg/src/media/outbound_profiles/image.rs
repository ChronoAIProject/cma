/// Inspect source bit depth/color without decoding pixels or inflating metadata.
/// The platform remains authoritative for full validity and client acceptance.
pub(crate) fn image_header(bytes: &[u8], mime: &str) -> bool {
    let dimensions = |width: u32, height: u32| {
        width > 0
            && height > 0
            && width.max(height) <= crate::media::image::MAX_SIDE
            && u64::from(width) * u64::from(height) <= crate::media::image::MAX_PIXELS
    };
    if mime == "image/png" {
        return bytes.get(8..16) == Some(&b"\0\0\0\rIHDR"[..])
            && bytes.get(24) == Some(&8)
            && matches!(bytes.get(25), Some(2 | 6))
            && bytes.get(16..24).is_some_and(|size| {
                dimensions(
                    u32::from_be_bytes(size[..4].try_into().unwrap()),
                    u32::from_be_bytes(size[4..].try_into().unwrap()),
                )
            });
    }
    let mut offset = 2;
    while bytes.get(offset) == Some(&0xff) {
        while bytes.get(offset) == Some(&0xff) {
            offset += 1;
        }
        let Some(&marker) = bytes.get(offset) else {
            return false;
        };
        offset += 1;
        if matches!(marker, 0xda | 0xd9 | 0x00) {
            return false;
        }
        let Some(length) = bytes.get(offset..offset + 2) else {
            return false;
        };
        let length = usize::from(u16::from_be_bytes([length[0], length[1]]));
        if length < 2 {
            return false;
        }
        let Some(segment) = bytes.get(offset + 2..offset + length) else {
            return false;
        };
        if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
            return matches!(marker, 0xc0 | 0xc2)
                && segment.len() >= 15
                && segment[0] == 8
                && segment[5] == 3
                && dimensions(
                    u32::from(u16::from_be_bytes([segment[3], segment[4]])),
                    u32::from(u16::from_be_bytes([segment[1], segment[2]])),
                );
        }
        offset += length;
    }
    false
}
