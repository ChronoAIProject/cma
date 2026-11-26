use super::*;
use std::{future::Future, io::Write, time::Duration};
use tokio::time::Instant;

pub(crate) const MAX_INITIAL_BYTES: usize = 8 * 1024 * 1024;

pub(super) async fn respond<T: serde::Serialize>(
    future: impl Future<Output = Result<(bool, T)>>,
) -> Response {
    let deadline = Instant::now() + Duration::from_secs(5);
    let result = tokio::time::timeout_at(deadline, async {
        let (pending, value) = future.await?;
        let bytes = encode(&value, MAX_INITIAL_BYTES, deadline)?;
        Ok((pending, bytes))
    })
    .await
    .unwrap_or(Err(MetricsError::Unavailable));
    match result {
        Ok((pending, bytes)) => {
            let status = if pending {
                StatusCode::ACCEPTED
            } else {
                StatusCode::OK
            };
            let mut response =
                (status, [(header::CONTENT_TYPE, "application/json")], bytes).into_response();
            if pending {
                response
                    .headers_mut()
                    .insert(header::RETRY_AFTER, "2".parse().unwrap());
            }
            response
        }
        Err(error) => problem(error),
    }
}

fn encode(value: &impl serde::Serialize, limit: usize, deadline: Instant) -> Result<Vec<u8>> {
    let mut writer = BoundedBody {
        bytes: Vec::new(),
        limit,
        deadline,
    };
    writer.check(0).map_err(|_| MetricsError::Unavailable)?;
    let value = wire::try_value(value).map_err(|_| MetricsError::Unavailable)?;
    serde_json::to_writer(&mut writer, &value).map_err(|_| MetricsError::Unavailable)?;
    writer.check(0).map_err(|_| MetricsError::Unavailable)?;
    Ok(writer.bytes)
}

struct BoundedBody {
    bytes: Vec<u8>,
    limit: usize,
    deadline: Instant,
}
impl BoundedBody {
    fn check(&self, incoming: usize) -> std::io::Result<()> {
        if incoming > self.limit.saturating_sub(self.bytes.len()) || Instant::now() >= self.deadline
        {
            return Err(std::io::Error::other("initial response limit exceeded"));
        }
        Ok(())
    }
}
impl Write for BoundedBody {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.check(bytes.len())?;
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.check(0)
    }
}

#[cfg(test)]
mod tests;
