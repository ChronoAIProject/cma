//! Validate complete media against Meta's signed digest without exceeding caller budgets.
use crate::media::{
    body::{read_capped, transport, FetchedBody, Overflow, SourceStream},
    FetchError,
};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tokio::time::{timeout_at, Instant};

pub(super) async fn fetch(
    response: reqwest::Response,
    cap: u64,
    overflow: Overflow,
    deadline: Instant,
    size: u64,
    digest: [u8; 32],
) -> Result<FetchedBody, FetchError> {
    let body = read_capped(response, cap.min(size), overflow, deadline).await?;
    // Excerpts remain bounded to the caller's read budget. Their authenticated
    // metadata matches the signed digest; only complete bodies can verify bytes.
    if body.truncated && size <= cap {
        return Err(FetchError::Rejected);
    }
    if !body.truncated
        && (body.bytes.len() as u64 != size
            || Sha256::digest(&body.bytes).as_slice() != &digest[..])
    {
        return Err(FetchError::Rejected);
    }
    Ok(body)
}

pub(super) fn open(
    response: reqwest::Response,
    cap: u64,
    deadline: Instant,
    size: u64,
    digest: [u8; 32],
) -> Result<SourceStream, FetchError> {
    if size > cap {
        return Err(FetchError::TooLarge);
    }
    let stream = futures_util::stream::unfold(
        (response.bytes_stream().boxed(), 0u64, Sha256::new(), false),
        move |(mut inner, seen, mut hash, ended)| async move {
            if ended {
                return None;
            }
            let next = timeout_at(deadline, inner.next()).await;
            let error = match next {
                Ok(Some(Ok(chunk))) => {
                    let total = seen.saturating_add(chunk.len() as u64);
                    if total <= size {
                        hash.update(&chunk);
                        return Some((Ok(chunk), (inner, total, hash, false)));
                    }
                    FetchError::Rejected
                }
                Ok(None) if seen == size && hash.clone().finalize().as_slice() == &digest[..] => {
                    return None
                }
                Ok(None) => FetchError::Rejected,
                Ok(Some(Err(error))) => transport(error),
                Err(_) => FetchError::TimedOut,
            };
            Some((Err(error), (inner, seen, hash, true)))
        },
    );
    Ok(SourceStream {
        content_length: Some(size),
        stream: stream.boxed(),
    })
}
