use super::*;

fn file(samples: &[i16]) -> PreparedFile {
    let mut bytes = BytesMut::zeroed(pcm::WAV_HEADER_BYTES);
    pcm::write_wav_header(&mut bytes, (samples.len() * 2) as u32);
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    PreparedFile {
        bytes: bytes.to_vec(),
        filename: "voice.wav".into(),
        mime: "audio/wav".into(),
        caption: None,
    }
}

#[test]
fn duration_and_entire_waveform_come_from_exact_pcm_samples() {
    let mut samples = vec![0; 1600];
    samples.extend(vec![i16::MIN; 1600]);
    samples.extend(vec![16384; 1600]);
    let result = metadata(&file(&samples)).unwrap();
    assert_eq!(result.duration_secs, 0.3);
    assert_eq!(STANDARD.decode(result.waveform).unwrap(), [0, 255, 128]);
    let mut samples = vec![0; MAX_SECONDS * 16_000];
    *samples.last_mut().unwrap() = i16::MIN;
    let result = metadata(&file(&samples)).unwrap();
    let waveform = STANDARD.decode(result.waveform).unwrap();
    assert_eq!(waveform.len(), 256);
    assert_eq!(waveform.last(), Some(&255));
    assert_eq!(result.duration_secs, 120.0);
}

#[test]
fn unsupported_headers_compressed_audio_trailing_bytes_and_claimed_formats_are_refused() {
    let good = file(&[0; 1600]);
    for index in [0, 4, 8, 12, 16, 20, 22, 24, 28, 32, 34, 36, 40] {
        let mut candidate = file(&[0; 1600]);
        candidate.bytes[index] ^= 1;
        assert!(metadata(&candidate).is_none(), "header byte {index}");
    }
    let mut trailing = file(&[0; 1600]);
    trailing.bytes.extend([0, 0]);
    assert!(metadata(&trailing).is_none());
    assert!(metadata(&file(&[0; 1599])).is_none());
    let oversized_samples = vec![0; MAX_SECONDS * 16000 + 1];
    assert!(metadata(&file(&oversized_samples)).is_none());
    let mut compressed = good;
    compressed.bytes = include_bytes!("../../tests/fixtures/media/voice.ogg").to_vec();
    assert!(metadata(&compressed).is_none());
    let mut mime = file(&[0; 1600]);
    mime.mime = "audio/ogg".into();
    assert!(metadata(&mime).is_none());
    mime.mime = "audio/wav".into();
    mime.caption = Some("caption".into());
    assert!(metadata(&mime).is_none());
}
