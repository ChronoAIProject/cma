use super::*;
use crate::discord::voice::test_support::{decoder, Output, OGG};
use base64::{engine::general_purpose::STANDARD, Engine};

fn file() -> PreparedFile {
    PreparedFile {
        bytes: OGG.to_vec(),
        mime: "audio/ogg".into(),
        filename: "original.ogg".into(),
        caption: None,
    }
}
fn channel() -> ChannelId {
    ChannelId::new("cmaeg1.ch.voice-test").unwrap()
}

#[tokio::test]
async fn opus_upload_retains_exact_bytes_and_derives_metadata_with_shared_bounded_decoder() {
    let (decoder, stub, budget) = decoder(Output::Good);
    let before = budget.available_bytes();
    let prepared = prepare(file(), true, true, &channel(), Some(&decoder))
        .await
        .unwrap();
    assert_eq!(prepared.file().bytes, OGG);
    assert_eq!(prepared.file().mime, "audio/ogg");
    assert_eq!(prepared.file().filename, "voice.ogg");
    assert_eq!(prepared.metadata().duration_secs, 0.3);
    assert_eq!(
        STANDARD.decode(&prepared.metadata().waveform).unwrap(),
        [0, 255, 128]
    );
    assert_eq!(
        *stub.calls.lock().unwrap(),
        [(channel().to_string(), OGG.to_vec(), 120)]
    );
    assert!(budget.available_bytes() < before);
    drop(prepared);
    assert_eq!(budget.available_bytes(), before);
}

#[tokio::test]
async fn incomplete_oversized_or_failed_decode_never_becomes_voice_authority() {
    for outcome in [
        Output::Truncated,
        Output::AtLimit,
        Output::OverLimit,
        Output::Invalid,
        Output::Failed,
    ] {
        let (decoder, stub, budget) = decoder(outcome);
        let before = budget.available_bytes();
        assert!(prepare(file(), false, false, &channel(), Some(&decoder))
            .await
            .is_none());
        assert_eq!(stub.calls.lock().unwrap().len(), 1);
        assert_eq!(budget.available_bytes(), before);
    }
    assert!(prepare(file(), false, false, &channel(), None)
        .await
        .is_none());
}

#[tokio::test]
async fn unsupported_streams_and_insufficient_budget_refuse_before_decoding() {
    let (mut decoder, stub, _) = decoder(Output::Good);
    for defect in ["stereo", "chained", "crc", "mime", "caption"] {
        let mut file = file();
        match defect {
            "stereo" => {
                let offset = file
                    .bytes
                    .windows(8)
                    .position(|bytes| bytes == b"OpusHead")
                    .unwrap();
                file.bytes[offset + 9] = 2;
                ogg::repair_checksums(&mut file.bytes);
            }
            "chained" => file.bytes.extend_from_slice(OGG),
            "crc" => file.bytes[22] ^= 1,
            "mime" => file.mime = "audio/wav".into(),
            "caption" => file.caption = Some("caption".into()),
            _ => unreachable!(),
        }
        assert!(
            prepare(file, false, false, &channel(), Some(&decoder))
                .await
                .is_none(),
            "{defect}"
        );
    }
    decoder.max_inflight = 2 * OGG.len() as u64;
    assert!(prepare(file(), false, false, &channel(), Some(&decoder))
        .await
        .is_none());
    assert!(stub.calls.lock().unwrap().is_empty());
}
