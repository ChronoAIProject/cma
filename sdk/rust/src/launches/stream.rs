//! Read-only, owner-scoped Launch SSE observation.

use super::{LaunchEvent, Launches};
use crate::{
    Error, Result,
    http::Request,
    resources::identity,
    stream::{self, EventStream},
};

/// One bounded replay page or an explicitly followed Launch event stream.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LaunchReplayOptions {
    /// Last complete durable event acknowledged by the caller.
    pub after: Option<u64>,
    /// Maximum events per backend read, from 1 through 200.
    pub limit: Option<u16>,
    /// Follow later events; false closes after one captured page.
    pub follow: bool,
}

/// Pull-based observer with an exact durable Launch cursor.
pub struct LaunchStream {
    inner: EventStream,
    launch_id: String,
    cursor: u64,
    ended: bool,
}

impl LaunchStream {
    /// Read one complete event; malformed frames stop observation without advancing the cursor.
    pub async fn next(&mut self) -> Option<Result<LaunchEvent>> {
        if self.ended {
            return None;
        }
        let frame = self.inner.next().await?;
        let result = frame.and_then(|frame| {
            let event: LaunchEvent = serde_json::from_value(frame.data)
                .map_err(|_| Error::protocol("invalid Launch event document"))?;
            identity(&event.launch_id, &self.launch_id)?;
            if frame.event != "message"
                || frame.last_event_id != event.seq.to_string()
                || event.seq <= self.cursor
            {
                return Err(Error::protocol(
                    "Launch event name, identity, or cursor did not match its envelope",
                ));
            }
            self.cursor = event.seq;
            Ok(event)
        });
        if result.is_err() {
            self.ended = true;
        }
        Some(result)
    }

    /// Last complete durable event yielded by this reader.
    pub fn cursor(&self) -> u64 {
        self.cursor
    }
}

impl Launches<'_> {
    /// Observe durable Launch progress without admitting, starting, cancelling, or retrying work.
    pub async fn stream(&self, id: &str, options: LaunchReplayOptions) -> Result<LaunchStream> {
        if options
            .limit
            .is_some_and(|limit| !(1..=200).contains(&limit))
        {
            return Err(Error::validation("Launch event limit must be 1 to 200"));
        }
        let mut query = vec![("follow", options.follow.to_string())];
        if let Some(after) = options.after {
            query.push(("starting_after", after.to_string()));
        }
        if let Some(limit) = options.limit {
            query.push(("limit", limit.to_string()));
        }
        let segments = ["agui", "launches", id, "events"];
        let mut request = Request::get(&segments);
        request.query = &query;
        Ok(LaunchStream {
            inner: stream::open(&self.client.inner, request).await?,
            launch_id: id.into(),
            cursor: options.after.unwrap_or(0),
            ended: false,
        })
    }
}
