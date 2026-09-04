use axum::body::Bytes;
use futures_util::{Stream, StreamExt as _};

struct Guard<S> {
    stream: S,
    credential: Vec<u8>,
    pending: Vec<u8>,
    ended: bool,
}

pub(crate) fn guarded<S>(
    stream: S,
    credential: Vec<u8>,
) -> impl Stream<Item = Result<Bytes, std::io::Error>>
where
    S: Stream<Item = Result<Bytes, reqwest::Error>> + Unpin,
{
    futures_util::stream::try_unfold(
        Guard {
            stream,
            credential,
            pending: Vec::new(),
            ended: false,
        },
        |mut state| async move {
            loop {
                if state.ended {
                    if state.pending.is_empty() {
                        return Ok(None);
                    }
                    if contains(&state.pending, &state.credential) {
                        return Err(reflection());
                    }
                    let tail = std::mem::take(&mut state.pending);
                    return Ok(Some((Bytes::from(tail), state)));
                }
                match state.stream.next().await {
                    Some(Ok(chunk)) => state.pending.extend_from_slice(&chunk),
                    Some(Err(error)) => {
                        return Err(std::io::Error::other(format!(
                            "upstream response stream failed: {error}"
                        )))
                    }
                    None => {
                        state.ended = true;
                        continue;
                    }
                }
                if contains(&state.pending, &state.credential) {
                    return Err(reflection());
                }
                let retained = state.credential.len().saturating_sub(1);
                if state.pending.len() <= retained {
                    continue;
                }
                let emit = state.pending.len() - retained;
                let rest = state.pending.split_off(emit);
                let bytes = std::mem::replace(&mut state.pending, rest);
                return Ok(Some((Bytes::from(bytes), state)));
            }
        },
    )
}

pub(crate) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty()
        && haystack
            .windows(needle.len())
            .any(|window| window == needle)
}

fn reflection() -> std::io::Error {
    tracing::error!("upstream response contained protected credential material");
    std::io::Error::other("upstream response was refused at the credential boundary")
}

#[cfg(test)]
mod tests {
    use futures_util::TryStreamExt as _;

    use super::*;

    #[tokio::test]
    async fn a_credential_split_across_chunks_is_never_emitted() {
        let source = futures_util::stream::iter([
            Ok::<_, reqwest::Error>(Bytes::from_static(b"before-sec")),
            Ok(Bytes::from_static(b"ret-after")),
        ]);
        let result = guarded(source, b"secret".to_vec())
            .try_collect::<Vec<_>>()
            .await;
        assert!(result.is_err());
    }
}
