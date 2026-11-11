//! Keep read admission charged while a transport retains the returned bytes.
use std::convert::Infallible;

use axum::body::{Body, Bytes};
use futures_util::stream;
use tokio::sync::OwnedSemaphorePermit;

pub(super) fn body(bytes: Bytes, permit: OwnedSemaphorePermit, enabled: bool) -> Body {
    if !enabled {
        return Body::from_stream(stream::unfold(
            (Some(bytes), permit),
            |(bytes, permit)| async move {
                bytes.map(|bytes| (Ok::<_, Infallible>(bytes), (None, permit)))
            },
        ));
    }
    if bytes.is_empty() {
        drop(permit);
        return Body::empty();
    }
    // Body EOF/drop may precede transport completion. Bytes owns the permit,
    // including across zero-copy clones/slices, until the final owner drops.
    Body::from(Bytes::from_owner(RetainedResponse {
        bytes,
        _permit: permit,
    }))
}

struct RetainedResponse {
    bytes: Bytes,
    _permit: OwnedSemaphorePermit,
}

impl AsRef<[u8]> for RetainedResponse {
    fn as_ref(&self) -> &[u8] {
        &self.bytes
    }
}
