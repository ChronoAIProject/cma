use super::*;

pub(crate) fn fixtures() -> [(&'static str, &'static [u8]); 5] {
    [
        (
            "audio/aac",
            include_bytes!("../../../tests/fixtures/whatsapp-outbound/whatsapp-tone.aac"),
        ),
        (
            "audio/amr",
            include_bytes!("../../../tests/fixtures/media/tone.amr"),
        ),
        (
            "audio/mpeg",
            include_bytes!("../../../tests/fixtures/media/tone.mp3"),
        ),
        (
            "audio/mp4",
            include_bytes!("../../../tests/fixtures/whatsapp-outbound/whatsapp-tone.m4a"),
        ),
        (
            "audio/ogg",
            include_bytes!("../../../tests/fixtures/media/voice.ogg"),
        ),
    ]
}

pub(crate) fn stereo_ogg() -> Vec<u8> {
    let mut bytes = fixtures()[4].1.to_vec();
    let head = bytes
        .windows(8)
        .position(|value| value == b"OpusHead")
        .unwrap();
    bytes[head + 9] = 2;
    ogg::repair_checksums(&mut bytes);
    bytes
}

#[test]
fn native_whatsapp_audio_recognizes_real_codec_fixtures_without_runtime_binaries() {
    for (mime, bytes) in fixtures() {
        assert_eq!(inspect(bytes).map(Format::mime), Some(mime), "{mime}");
        assert!(
            inspect(&bytes[..bytes.len() - 1]).is_none(),
            "truncated {mime}"
        );
        // A complete first MPEG/ADTS/AMR frame is independently valid, so prefixes
        // need not all refuse; every bounded prefix must be safe to inspect.
        for end in 0..bytes.len().min(512) {
            let _ = inspect(&bytes[..end]);
        }
    }
}

#[test]
fn native_whatsapp_audio_refuses_mislabeled_non_audio_and_unsupported_codecs() {
    for bytes in [
        &b"not an audio container"[..],
        &b"RIFF\0\0\0\0WAVE"[..],
        include_bytes!("../../../tests/fixtures/media/h264_aac.mp4"),
        include_bytes!("../../../tests/fixtures/media/vp9_opus.webm"),
        &b"#!AMR_MC1.0\n"[..],
        &b"#!AMR-WB\n"[..],
    ] {
        assert!(inspect(bytes).is_none());
    }
    let mut aac = fixtures()[0].1.to_vec();
    aac[2] &= 0x3f; // AAC Main, outside the admitted AAC-LC profile.
    assert!(inspect(&aac).is_none());
    let mut mp3 = without_id3(fixtures()[2].1).unwrap().to_vec();
    mp3[1] = (mp3[1] & !6) | 4; // MPEG Layer II is not MP3.
    assert!(inspect(&mp3).is_none());
    let mut amr = fixtures()[1].1.to_vec();
    amr[6] = 9 << 3; // Reserved narrowband frame type.
    assert!(inspect(&amr).is_none());
}

#[test]
fn native_whatsapp_audio_ogg_requires_mono_opus_one_stream_and_valid_pages() {
    let bytes = fixtures()[4].1;
    let head = bytes
        .windows(8)
        .position(|value| value == b"OpusHead")
        .unwrap();
    for defect in 0..5 {
        let mut changed = bytes.to_vec();
        match defect {
            0 => changed[head + 9] = 2,
            1 => changed[head + 18] = 1,
            2 => changed[head] = b'V',
            3 => changed[18..22].copy_from_slice(&1u32.to_le_bytes()),
            4 => changed[5] &= !2,
            _ => unreachable!(),
        }
        ogg::repair_checksums(&mut changed);
        assert!(inspect(&changed).is_none(), "defect {defect}");
    }
    let mut chained = bytes.to_vec();
    chained.extend_from_slice(bytes);
    assert!(inspect(&chained).is_none());
    let mut damaged = bytes.to_vec();
    *damaged.last_mut().unwrap() ^= 1;
    assert!(inspect(&damaged).is_none());
}

#[test]
fn native_whatsapp_audio_mp4_requires_self_contained_aac_and_bounded_boxes() {
    let original = fixtures()[3].1;
    for (marker, replace) in [(b"mp4a", b"alac"), (b"soun", b"vide")] {
        let mut bytes = original.to_vec();
        let start = bytes.windows(4).position(|value| value == marker).unwrap();
        bytes[start..start + 4].copy_from_slice(replace);
        assert!(inspect(&bytes).is_none());
    }
    let mut bytes = original.to_vec();
    let url = bytes.windows(4).position(|value| value == b"url ").unwrap();
    bytes[url + 7] = 0; // dref is no longer self-contained.
    assert!(inspect(&bytes).is_none());
    let mut bytes = original.to_vec();
    bytes[..4].copy_from_slice(&1u32.to_be_bytes());
    bytes[8..16].copy_from_slice(&u64::MAX.to_be_bytes());
    assert!(inspect(&bytes).is_none());
    let mut bytes = original.to_vec();
    let size = bytes.windows(4).position(|value| value == b"stsz").unwrap();
    bytes[size + 12..size + 16].copy_from_slice(&u32::MAX.to_be_bytes());
    assert!(inspect(&bytes).is_none());
    let mut bytes = original.to_vec();
    let offset = bytes.windows(4).position(|value| value == b"stco").unwrap();
    let end = bytes.len() as u32 - 1;
    bytes[offset + 12..offset + 16].copy_from_slice(&end.to_be_bytes());
    assert!(inspect(&bytes).is_none()); // Chunk starts inside mdat but runs beyond it.
    let mut bytes = original.to_vec();
    let mapping = bytes.windows(4).position(|value| value == b"stsc").unwrap();
    bytes[mapping + 16..mapping + 20].copy_from_slice(&1u32.to_be_bytes());
    assert!(inspect(&bytes).is_none()); // Every declared sample must belong to a chunk.
}

#[test]
fn native_whatsapp_audio_size_and_adversarial_inputs_remain_bounded() {
    let mut bytes = fixtures()[0].1.to_vec();
    bytes.resize(crate::media::outbound::MAX_OUTBOUND_BYTES as usize + 1, 0);
    assert!(inspect(&bytes).is_none());
    let mut state = 17u32;
    for size in 0..512 {
        let mut bytes = vec![0; size];
        for byte in &mut bytes {
            state = state.wrapping_mul(1664525).wrapping_add(1013904223);
            *byte = (state >> 24) as u8;
        }
        let _ = inspect(&bytes);
        for prefix in [&b"OggS"[..], &b"ID3\x04\x00\x00"[..], &b"#!AMR\n"[..]] {
            if bytes.len() >= prefix.len() {
                bytes[..prefix.len()].copy_from_slice(prefix);
                let _ = inspect(&bytes);
            }
        }
    }
}
