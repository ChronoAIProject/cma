//! Inline custom attachments: `data:` URIs carried inside the verified event.
//!
//! The bytes are already in the sealed activity, so nothing is sent anywhere.
//! Ingress admitted only well-formed items; this re-reads just enough of the
//! URI to find its base64 payload and still refuses anything else.
use super::{FetchRequest, MediaSource, SourceStream};
use crate::{
    ingress::custom::attachments::{decoded_length, MAX_INLINE_ATTACHMENT_BYTES},
    media::{body::FetchedBody, FetchError},
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use bytes::Bytes;
use cmaeg_core::activity::{ActivityKind, AttachmentSource};
use futures_util::{future::BoxFuture, StreamExt};

/// Reads `inline{pointer}` locators from the activity's CloudEvent data.
#[derive(Debug, Default, Clone, Copy)]
pub struct InlineSource;

impl InlineSource {
    fn read(request: &FetchRequest<'_>) -> Result<Bytes, FetchError> {
        let Some(AttachmentSource::Inline { pointer }) = request.locator else {
            return Err(FetchError::NotRetrievable);
        };
        let ActivityKind::Event { ce, .. } = &request.activity.kind else {
            return Err(FetchError::NotRetrievable);
        };
        let uri = ce
            .data
            .as_ref()
            .and_then(|data| data.pointer(pointer))
            .and_then(|value| value.as_str())
            .ok_or(FetchError::NotFound)?;
        let payload = base64_payload(uri).ok_or(FetchError::NotFound)?;
        // The size is known before decoding, so an oversized payload is
        // refused without allocating for it.
        let decoded = decoded_length(payload).ok_or(FetchError::NotFound)?;
        let limit = request.cap.min(MAX_INLINE_ATTACHMENT_BYTES as u64);
        if decoded as u64 > limit {
            return Err(FetchError::TooLarge);
        }
        STANDARD
            .decode(payload)
            .map(Bytes::from)
            .map_err(|_| FetchError::NotFound)
    }
}

/// The payload of `data:<type>/<subtype>[;params];base64,<payload>`.
fn base64_payload(uri: &str) -> Option<&str> {
    let (header, payload) = uri.strip_prefix("data:")?.split_once(',')?;
    let mime = header.strip_suffix(";base64")?.split(';').next()?;
    let (kind, subtype) = mime.split_once('/')?;
    (!kind.is_empty() && !subtype.is_empty()).then_some(payload)
}

impl MediaSource for InlineSource {
    fn fetch<'a>(
        &'a self,
        request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<FetchedBody, FetchError>> {
        Box::pin(async move {
            Self::read(&request).map(|bytes| FetchedBody {
                bytes,
                truncated: false,
            })
        })
    }

    fn open<'a>(
        &'a self,
        request: FetchRequest<'a>,
    ) -> BoxFuture<'a, Result<SourceStream, FetchError>> {
        Box::pin(async move {
            let bytes = Self::read(&request)?;
            Ok(SourceStream {
                content_length: Some(bytes.len() as u64),
                stream: futures_util::stream::once(async move { Ok(bytes) }).boxed(),
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_base64_data_uris_with_a_media_type_have_a_payload() {
        assert_eq!(base64_payload("data:image/png;base64,AAAA"), Some("AAAA"));
        assert_eq!(
            base64_payload("data:text/plain;charset=utf-8;base64,aGk="),
            Some("aGk=")
        );
        for uri in [
            "https://media.example/a.png",
            "data:image/png,AAAA",
            "data:;base64,AAAA",
            "data:image;base64,AAAA",
            "data:/png;base64,AAAA",
            "data:image/png;base64",
        ] {
            assert_eq!(base64_payload(uri), None, "{uri}");
        }
    }
}
