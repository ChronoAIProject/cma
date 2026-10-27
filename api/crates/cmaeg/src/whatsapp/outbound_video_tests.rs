use super::super::bmff::sample_ranges;
use super::*;

pub(crate) fn fixtures() -> [(&'static str, &'static [u8]); 2] {
    [
        (
            "video/mp4",
            include_bytes!("../../tests/fixtures/whatsapp-outbound/whatsapp-baseline.mp4"),
        ),
        (
            "video/3gpp",
            include_bytes!("../../tests/fixtures/whatsapp-outbound/whatsapp-baseline-aac.3gp"),
        ),
    ]
}
fn position(bytes: &[u8], kind: &[u8; 4]) -> usize {
    bytes.windows(4).position(|bytes| bytes == kind).unwrap() + 4
}
fn video_ranges(bytes: &[u8]) -> Vec<std::ops::Range<usize>> {
    let top = atoms(bytes, 0).unwrap();
    let movie = child(one(&top, b"moov").unwrap()).unwrap();
    let timing = container::movie(one(&movie, b"mvhd").unwrap()).unwrap();
    let track = container::track(
        movie.iter().find(|atom| atom.kind == *b"trak").unwrap(),
        timing,
    )
    .unwrap();
    sample_ranges(&top, &track.table, MAX_SAMPLES).unwrap()
}
fn nals(bytes: &[u8]) -> Vec<std::ops::Range<usize>> {
    let mut result = Vec::new();
    for sample in video_ranges(bytes) {
        let mut offset = sample.start;
        while offset < sample.end {
            let size = u32_at(bytes, offset).unwrap() as usize;
            result.push(offset + 4..offset + 4 + size);
            offset += 4 + size;
        }
    }
    result
}
fn write(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

#[test]
fn native_whatsapp_video_real_baseline_fixtures_and_brand_mime() {
    for (mime, bytes) in fixtures() {
        assert_eq!(inspect(bytes).unwrap().mime(), mime);
    }
    // Container classification comes from the major brand, not generic MP4 sniffing.
    let mut bytes = fixtures()[0].1.to_vec();
    bytes[8..12].copy_from_slice(b"3gp6");
    assert_eq!(inspect(&bytes), Some(Format::ThreeGpp));
    bytes[8..12].copy_from_slice(b"M4A ");
    assert_eq!(inspect(&bytes), None);
}
#[test]
fn native_whatsapp_video_refuses_other_codecs_and_misleading_profile_labels() {
    for bytes in [
        &include_bytes!("../../tests/fixtures/media/h264_aac.mp4")[..],
        &include_bytes!("../../tests/fixtures/media/hevc_4k.mp4")[..],
        &include_bytes!("../../tests/fixtures/media/vp9_opus.webm")[..],
        &include_bytes!("../../tests/fixtures/whatsapp-outbound/whatsapp-tone.m4a")[..],
    ] {
        assert!(inspect(bytes).is_none());
    }
    let mut high = include_bytes!("../../tests/fixtures/media/h264_aac.mp4").to_vec();
    let avcc = position(&high, b"avcC");
    high[avcc + 1] = 66;
    high[avcc + 9] = 66; // SPS profile too; high-profile syntax is still refused.
    assert!(inspect(&high).is_none());
    let original = fixtures()[0].1;
    let p = nals(original)
        .into_iter()
        .find(|range| original[range.start] & 31 == 1)
        .unwrap();
    let mut bytes = original.to_vec();
    assert_eq!((bytes[p.start + 1] >> 2) & 31, 6); // first_mb=0, slice_type=5 (P)
    bytes[p.start + 1] |= 4; // same-width Exp-Golomb 6 (B), with Baseline headers unchanged
    assert!(inspect(&bytes).is_none());
}
#[test]
fn native_whatsapp_video_checks_every_sample_and_refuses_truncation() {
    for (_, original) in fixtures() {
        for end in 0..original.len() {
            assert!(inspect(&original[..end]).is_none(), "prefix {end}");
        }
        for range in video_ranges(original) {
            let mut bytes = original.to_vec();
            write(&mut bytes, range.start, u32::MAX);
            assert!(inspect(&bytes).is_none(), "sample {}", range.start);
        }
        for range in nals(original) {
            let mut bytes = original.to_vec();
            bytes[range.start] |= 0x80;
            assert!(inspect(&bytes).is_none(), "NAL {}", range.start);
        }
        let mut bytes = original.to_vec();
        bytes.push(0);
        assert!(inspect(&bytes).is_none());
    }
}
#[test]
fn native_whatsapp_video_refuses_container_and_timing_ambiguity() {
    for defect in 0..21 {
        let mut bytes = fixtures()[0].1.to_vec();
        match defect {
            0 => {
                let p = position(&bytes, b"url ");
                bytes[p + 3] = 0;
            }
            1 => {
                let p = position(&bytes, b"free");
                bytes[p - 4..p].copy_from_slice(b"moof");
            }
            2 => {
                let p = position(&bytes, b"avc1"); // skip compatible brand, choose actual sample entry
                let p = bytes[p..]
                    .windows(4)
                    .position(|part| part == b"avc1")
                    .unwrap()
                    + p;
                bytes[p..p + 4].copy_from_slice(b"encv");
            }
            3 => {
                let p = position(&bytes, b"avcC");
                bytes[p - 4..p].copy_from_slice(b"sinf");
            }
            4 => {
                let p = position(&bytes, b"hdlr");
                bytes[p + 8..p + 12].copy_from_slice(b"hint");
            }
            5 => {
                let p = position(&bytes, b"stco");
                write(&mut bytes, p + 8, 0);
            }
            6 => {
                let p = position(&bytes, b"stco");
                write(&mut bytes, p + 8, u32::MAX);
            }
            7 => {
                let p = position(&bytes, b"stsc");
                write(&mut bytes, p + 12, u32::MAX);
            }
            8 => {
                let p = position(&bytes, b"stsz");
                write(&mut bytes, p + 8, u32::MAX);
            }
            9 => {
                let p = position(&bytes, b"stts");
                write(&mut bytes, p + 12, 0);
            }
            10 => {
                let p = position(&bytes, b"stts");
                write(&mut bytes, p + 8, 1);
            }
            11 => {
                let p = position(&bytes, b"stts");
                write(&mut bytes, p + 12, 1);
            }
            12 => {
                let p = position(&bytes, b"mdhd");
                write(&mut bytes, p + 12, 0);
            }
            13 => {
                let p = position(&bytes, b"stss");
                write(&mut bytes, p + 8, 2);
            }
            14 => {
                let p = position(&bytes, b"avcC");
                bytes[p + 1] = 100;
            }
            15 => {
                let p = position(&bytes, b"avcC");
                bytes[p + 4] = 0xfe;
            }
            16 => {
                let p = position(&bytes, b"avcC");
                bytes[p + 5] = 0xe2;
            }
            17 => {
                let p = position(&bytes, b"tkhd");
                write(&mut bytes, p + 76, 80 << 16);
            }
            18 => {
                let p = position(&bytes, b"avcC");
                bytes[p + 6..p + 8].copy_from_slice(&u16::MAX.to_be_bytes());
            }
            19 => {
                let p = position(&bytes, b"pasp");
                write(&mut bytes, p, 2);
            }
            20 => {
                let p = position(&bytes, b"mvhd");
                write(&mut bytes, p + 20, 2 << 16);
            }
            _ => unreachable!(),
        }
        assert!(inspect(&bytes).is_none(), "defect {defect}");
    }
}
#[test]
fn native_whatsapp_video_refuses_multiple_audio_tracks_overlap_and_non_faststart() {
    let original = fixtures()[1].1;
    let top = atoms(original, 0).unwrap();
    let movie = one(&top, b"moov").unwrap();
    let parts = child(movie).unwrap();
    let audio = parts
        .iter()
        .filter(|atom| atom.kind == *b"trak")
        .nth(1)
        .unwrap();
    let mut bytes = original.to_vec();
    let extra = original[audio.body_offset - 8..audio.body_offset + audio.body.len()].to_vec();
    let end = movie.body_offset + movie.body.len();
    bytes.splice(end..end, extra.iter().copied());
    write(
        &mut bytes,
        movie.body_offset - 8,
        (movie.body.len() + 8 + extra.len()) as u32,
    );
    assert!(inspect(&bytes).is_none());
    // Audio chunks cannot alias video data, even when each individual local span fits.
    let timing = container::movie(one(&parts, b"mvhd").unwrap()).unwrap();
    let audio = container::track(audio, timing).unwrap();
    let offsets = one(&audio.table, b"stco").unwrap();
    let mut bytes = original.to_vec();
    let video_start = video_ranges(original)[0].start as u32;
    write(&mut bytes, offsets.body_offset + 8, video_start);
    assert!(inspect(&bytes).is_none());
    let original = fixtures()[0].1;
    let top = atoms(original, 0).unwrap();
    let movie = one(&top, b"moov").unwrap();
    let mut bytes = original.to_vec();
    let moved: Vec<_> = bytes
        .drain(movie.body_offset - 8..movie.body_offset + movie.body.len())
        .collect();
    bytes.extend(moved);
    assert!(inspect(&bytes).is_none());
}
#[test]
fn native_whatsapp_video_rbsp_and_exp_golomb_bounds() {
    for nal in [
        &[0x67, 0, 0, 0][..],
        &[0x67, 0, 0, 1],
        &[0x67, 0, 0, 3],
        &[0x67, 0, 0, 3, 4],
    ] {
        assert!(bits::rbsp(nal, 7).is_none());
    }
    assert_eq!(
        bits::rbsp(&[0x67, 0, 0, 3, 1, 0x80], 7),
        Some(vec![0, 0, 1, 0x80])
    );
    assert!(bits::Bits::new(&[0; 64]).ue(u32::MAX).is_none());
    assert!(bits::Bits::new(&[0x80, 0]).trailing().is_none());
}

fn avcc_with_parameter_mutation(sps: bool, bit: usize) -> Vec<u8> {
    let bytes = fixtures()[0].1;
    let start = position(bytes, b"avcC");
    let length = u32_at(bytes, start - 8).unwrap() as usize - 8;
    let config = &bytes[start..start + length];
    let sps_len = usize::from(u16::from_be_bytes(config[6..8].try_into().unwrap()));
    let (offset, size) = if sps {
        (8, sps_len)
    } else {
        let p = 9 + sps_len;
        (
            p + 2,
            usize::from(u16::from_be_bytes(config[p..p + 2].try_into().unwrap())),
        )
    };
    let nal = &config[offset..offset + size];
    let mut rbsp = bits::rbsp(nal, if sps { 7 } else { 8 }).unwrap();
    rbsp[(bit - 8) / 8] ^= 1 << (7 - (bit - 8) % 8);
    let mut escaped = vec![nal[0]];
    let mut zeroes = 0;
    for byte in rbsp {
        if zeroes == 2 && byte <= 3 {
            escaped.push(3);
            zeroes = 0;
        }
        escaped.push(byte);
        zeroes = if byte == 0 { zeroes + 1 } else { 0 };
    }
    let mut result = config[..offset - 2].to_vec();
    result.extend_from_slice(&(escaped.len() as u16).to_be_bytes());
    result.extend(escaped);
    result.extend_from_slice(&config[offset + size..]);
    result
}
#[test]
fn native_whatsapp_video_rejects_unsupported_sps_pps_coding_modes() {
    // Syntax bit positions independently inspected with FFmpeg trace_headers.
    // SPS: reserved bit, POC type 1, frame gaps, interlace, nonsquare pixels, HRD, reordering.
    for bit in [23, 36, 42, 51, 62, 133, 154] {
        assert!(
            h264::Avc::parse(&avcc_with_parameter_mutation(true, bit), 64, 48).is_none(),
            "SPS bit {bit}"
        );
    }
    // PPS: CABAC, slice groups, weighted prediction, redundant pictures, extensions.
    for bit in [10, 12, 17, 33, 34] {
        assert!(
            h264::Avc::parse(&avcc_with_parameter_mutation(false, bit), 64, 48).is_none(),
            "PPS bit {bit}"
        );
    }
}
#[test]
fn native_whatsapp_video_timing_edits_and_audio_groups_cannot_bypass_limits() {
    for defect in 0..7 {
        let mut bytes = fixtures()[1].1.to_vec();
        match defect {
            0 => {
                let p = position(&bytes, b"mvhd");
                write(&mut bytes, p + 16, 600001);
            }
            1 => {
                let p = position(&bytes, b"tkhd");
                write(&mut bytes, p + 20, 600001);
            }
            2 => {
                let p = position(&bytes, b"elst");
                write(&mut bytes, p + 8, u32::MAX);
            }
            3 => {
                let p = position(&bytes, b"elst");
                write(&mut bytes, p + 12, 1000000);
            }
            4 => {
                let p = position(&bytes, b"sgpd");
                bytes[p + 4..p + 8].copy_from_slice(b"seig");
            }
            5 => {
                let p = position(&bytes, b"sbgp");
                write(&mut bytes, p + 16, 2);
            }
            6 => {
                let p = position(&bytes, b"esds"); // AAC object type is proven independently of the mp4a label.
                let asc = bytes[p..]
                    .windows(5)
                    .position(|part| part == [5, 128, 128, 128, 5])
                    .unwrap()
                    + p
                    + 5;
                bytes[asc] = (bytes[asc] & 7) | (5 << 3);
            }
            _ => unreachable!(),
        }
        assert!(inspect(&bytes).is_none(), "defect {defect}");
    }
}
#[test]
fn native_whatsapp_video_adversarial_bit_mutations_never_panic() {
    for (_, original) in fixtures() {
        for position in 0..original.len() {
            let mut bytes = original.to_vec();
            bytes[position] ^= 1 << (position % 8);
            let _ = inspect(&bytes);
        }
    }
}
