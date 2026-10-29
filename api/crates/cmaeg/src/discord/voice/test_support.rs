//! Deterministic decoder seam; the committed Ogg also runs in image self-tests.
use super::{Decoder, MAX_SECONDS};
use crate::media::{
    budget::InflightBytes,
    transform::{
        pcm, AudioRequest, KeyframeRequest, Keyframes, MediaTransformer, Pcm, TransformError,
    },
    MediaLimits,
};
use bytes::{Bytes, BytesMut};
use futures_util::future::BoxFuture;
use std::sync::{Arc, Mutex};

pub(crate) const OGG: &[u8] = include_bytes!("../../../tests/fixtures/media/voice.ogg");

#[derive(Clone, Copy)]
pub(crate) enum Output {
    Good,
    Truncated,
    AtLimit,
    OverLimit,
    Invalid,
    Failed,
}
pub(crate) struct Stub {
    pub calls: Mutex<Vec<(String, Vec<u8>, u16)>>,
    pub output: Output,
}
pub(crate) fn decoder(output: Output) -> (Decoder, Arc<Stub>, InflightBytes) {
    let stub = Arc::new(Stub {
        calls: Mutex::default(),
        output,
    });
    let limits = MediaLimits::from_config(&crate::MediaConfig::default());
    let inflight = InflightBytes::new(&limits);
    (
        Decoder::new(stub.clone(), inflight.clone(), limits.inflight_bytes),
        stub,
        inflight,
    )
}
impl MediaTransformer for Stub {
    fn keyframes(&self, _: KeyframeRequest) -> BoxFuture<'_, Result<Keyframes, TransformError>> {
        Box::pin(async { Err(TransformError::Unavailable) })
    }
    fn audio(&self, request: AudioRequest) -> BoxFuture<'_, Result<Pcm, TransformError>> {
        self.calls.lock().unwrap().push((
            request.channel.to_string(),
            request.input.to_vec(),
            request.seconds,
        ));
        Box::pin(async move {
            if matches!(self.output, Output::Failed) {
                return Err(TransformError::Unavailable);
            }
            let samples = match self.output {
                Output::AtLimit => MAX_SECONDS * 16_000,
                Output::OverLimit => MAX_SECONDS * 16_000 + 1,
                _ => 4_800,
            };
            let data_len = samples * 2;
            let mut wav = BytesMut::zeroed(pcm::WAV_HEADER_BYTES + data_len);
            pcm::write_wav_header(&mut wav, data_len as u32);
            // Three bins: quiet, full-negative amplitude, then half amplitude.
            for (index, sample) in wav[pcm::WAV_HEADER_BYTES..].chunks_exact_mut(2).enumerate() {
                sample.copy_from_slice(
                    &match index / 1600 {
                        0 => 0i16,
                        1 => i16::MIN,
                        _ => 16384i16,
                    }
                    .to_le_bytes(),
                );
            }
            if matches!(self.output, Output::Invalid) {
                wav[0] = 0;
            }
            Ok(Pcm {
                wav: Bytes::from(wav),
                data_len,
                truncated: matches!(self.output, Output::Truncated),
            })
        })
    }
}
