use super::*;

pub(crate) fn fixtures() -> [(&'static str, &'static [u8], bool); 3] {
    [
        (
            "static",
            include_bytes!("../../tests/fixtures/whatsapp-outbound/whatsapp-static.webp"),
            false,
        ),
        (
            "static-lossy",
            include_bytes!("../../tests/fixtures/whatsapp-outbound/whatsapp-static-lossy.webp"),
            false,
        ),
        (
            "animated",
            include_bytes!("../../tests/fixtures/whatsapp-outbound/whatsapp-animated.webp"),
            true,
        ),
    ]
}
fn riff_size(bytes: &mut [u8]) {
    let size = (bytes.len() - 8) as u32;
    bytes[4..8].copy_from_slice(&size.to_le_bytes());
}
fn chunk(kind: &[u8; 4], bytes: &[u8]) -> Vec<u8> {
    let mut out = kind.to_vec();
    out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(bytes);
    if bytes.len() & 1 != 0 {
        out.push(0);
    }
    out
}
fn metadata_padded(original: &[u8], total: usize) -> Vec<u8> {
    let mut bytes = original.to_vec();
    if &bytes[12..16] != b"VP8X" {
        let header = chunk(b"VP8X", &[0x10, 0, 0, 0, 0xff, 1, 0, 0xff, 1, 0]);
        bytes.splice(12..12, header);
    }
    bytes[20] |= 8;
    let padding = vec![0; total - bytes.len() - 8];
    bytes.extend(chunk(b"EXIF", &padding));
    riff_size(&mut bytes);
    bytes
}

#[test]
fn native_whatsapp_sticker_recognizes_real_static_and_animated_fixtures() {
    for (name, bytes, animated) in fixtures() {
        assert_eq!(inspect(bytes), Some(animated), "{name}");
        for end in 0..bytes.len() {
            assert!(inspect(&bytes[..end]).is_none(), "{name} prefix {end}");
        }
        let mut trailing = bytes.to_vec();
        trailing.extend_from_slice(b"extra");
        assert!(inspect(&trailing).is_none());
    }
}

#[test]
fn native_whatsapp_sticker_enforces_separate_static_animated_byte_limits() {
    for (source, limit, animated) in [
        (fixtures()[0].1, MAX_STATIC_BYTES, false),
        (fixtures()[2].1, MAX_ANIMATED_BYTES, true),
    ] {
        assert_eq!(inspect(&metadata_padded(source, limit)), Some(animated));
        assert!(inspect(&metadata_padded(source, limit + 2)).is_none());
    }
}

#[test]
fn native_whatsapp_sticker_refuses_wrong_dimensions_headers_and_unknown_chunks() {
    for defect in 0..7 {
        let mut bytes = fixtures()[1].1.to_vec();
        match defect {
            0 => bytes[24] = 0xfe,   // canvas width is 511.
            1 => bytes[20] |= 1,     // reserved VP8X flag.
            2 => bytes[21] = 1,      // reserved header byte.
            3 => bytes[20] &= !0x10, // frame alpha disagrees with canvas.
            4 => {
                let index = bytes.windows(4).position(|p| p == b"VP8 ").unwrap();
                bytes[index + 8] |= 1; // interframe VP8 is not a WebP image.
            }
            5 => {
                bytes.extend(chunk(b"JUNK", b"extra"));
                riff_size(&mut bytes);
            }
            6 => bytes[16..20].copy_from_slice(&u32::MAX.to_le_bytes()),
            _ => unreachable!(),
        }
        assert!(inspect(&bytes).is_none(), "defect {defect}");
    }
    let mut lossless = fixtures()[0].1.to_vec();
    let index = lossless.windows(4).position(|p| p == b"VP8L").unwrap();
    lossless[index + 12] |= 0x20; // unknown lossless version.
    assert!(inspect(&lossless).is_none());
}

#[test]
fn native_whatsapp_sticker_animation_checks_frame_bounds_timing_and_count() {
    let original = fixtures()[2].1;
    let first = original.windows(4).position(|p| p == b"ANMF").unwrap();
    for defect in 0..6 {
        let mut bytes = original.to_vec();
        match defect {
            0 => bytes[first + 8..first + 11].fill(0xff), // frame lies outside canvas.
            1 => bytes[first + 14] ^= 1, // declared frame width differs from image.
            2 => bytes[first + 20..first + 23].copy_from_slice(&[7, 0, 0]),
            3 => bytes[first + 20..first + 23].copy_from_slice(&[0x10, 0x27, 0]), // one frame already 10s.
            4 => bytes[first + 23] |= 4, // reserved frame flag.
            5 => bytes[20] &= !2,        // animated chunks without animation flag.
            _ => unreachable!(),
        }
        assert!(inspect(&bytes).is_none(), "defect {defect}");
    }
    let parts = chunks(&original[12..]).unwrap();
    let frame = parts.iter().find(|p| p.kind == b"ANMF").unwrap();
    let mut short = frame.data.to_vec();
    short[12..15].copy_from_slice(&[8, 0, 0]);
    let make = |count| {
        let mut bytes = original[..first].to_vec();
        for _ in 0..count {
            bytes.extend(chunk(b"ANMF", &short));
        }
        riff_size(&mut bytes);
        bytes
    };
    assert_eq!(inspect(&make(MAX_FRAMES)), Some(true));
    assert!(inspect(&make(MAX_FRAMES + 1)).is_none());
    assert!(inspect(&make(1)).is_none());
}

#[test]
fn native_whatsapp_sticker_adversarial_container_lengths_are_bounded() {
    let mut state = 17u32;
    for length in 12..1024 {
        let mut bytes = vec![0; length];
        for byte in &mut bytes {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            *byte = (state >> 24) as u8;
        }
        bytes[..4].copy_from_slice(b"RIFF");
        bytes[8..12].copy_from_slice(b"WEBP");
        riff_size(&mut bytes);
        assert!(inspect(&bytes).is_none());
    }
}
