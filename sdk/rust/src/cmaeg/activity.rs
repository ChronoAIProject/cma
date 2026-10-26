use super::{Cmaeg, types::ActivityPage};
use crate::{Error, Result, http::Request, stream::EventStream};
use reqwest::header::HeaderValue;

/// Owner selection for a recent Activity read.
#[derive(Default)]
pub struct ChannelActivityOptions<'a> {
    /// Organization subject; omitted selects the authenticated creator.
    pub owner: Option<&'a str>,
}

/// Explicit resume options for one Activity stream connection.
#[derive(Default)]
pub struct StreamChannelActivityOptions<'a> {
    /// Organization subject; omitted selects the authenticated creator.
    pub owner: Option<&'a str>,
    /// Opaque snapshot watermark sent as a query parameter.
    pub cursor: Option<&'a str>,
    /// Header alternative; takes precedence over cursor, as on the gateway.
    pub last_event_id: Option<&'a str>,
}

fn owner(request: &mut Request<'_>, value: Option<&str>) -> Result<()> {
    request.root = true;
    if let Some(value) = value {
        if value.is_empty() || value.len() > 512 || !value.bytes().all(|b| (33..=126).contains(&b))
        {
            return Err(Error::validation("invalid Activity owner"));
        }
        request.headers.insert(
            "x-cmaeg-owner",
            HeaderValue::from_str(value)
                .map_err(|_| Error::validation("invalid Activity owner"))?,
        );
    }
    Ok(())
}

impl Cmaeg {
    /// Read the owner's complete bounded recent Activity snapshot.
    pub async fn get_channel_activity(
        &self,
        id: &str,
        options: &ChannelActivityOptions<'_>,
    ) -> Result<ActivityPage> {
        let segments = ["v1", "channels", id, "activity"];
        let mut request = Request::get(&segments);
        owner(&mut request, options.owner)?;
        self.client.request(request).await
    }

    /// Observe snapshots once; reconnect explicitly with the last atomically applied cursor.
    pub async fn stream_channel_activity(
        &self,
        id: &str,
        options: &StreamChannelActivityOptions<'_>,
    ) -> Result<ChannelActivityStream> {
        let segments = ["v1", "channels", id, "activity", "stream"];
        let query: Vec<_> = options
            .cursor
            .map(|cursor| ("cursor", cursor.to_owned()))
            .into_iter()
            .collect();
        let mut request = Request::get(&segments);
        owner(&mut request, options.owner)?;
        request.query = &query;
        if let Some(cursor) = options.last_event_id {
            if cursor.is_empty()
                || cursor.len() > 8192
                || !cursor.bytes().all(|b| (33..=126).contains(&b))
            {
                return Err(Error::validation("invalid Activity last-event-id"));
            }
            request.headers.insert(
                "last-event-id",
                HeaderValue::from_str(cursor)
                    .map_err(|_| Error::validation("invalid Activity last-event-id"))?,
            );
        }
        let stream =
            crate::stream::open_bounded(&self.client.inner, request, 8 * 1024 * 1024).await?;
        Ok(ChannelActivityStream {
            stream: Some(stream),
            observed: false,
        })
    }
}

/// Both variants replace the entire recent Activity snapshot atomically.
#[derive(Clone, PartialEq)]
pub enum ChannelActivityEvent {
    /// Initial or updated complete snapshot; its cursor matches the explicit SSE ID.
    Activity(ActivityPage),
    /// Complete replacement after retention expiry or a missed notification.
    Reset(ActivityPage),
}

/// Bounded pull-based snapshots. Dropping the reader ends observation only.
pub struct ChannelActivityStream {
    stream: Option<EventStream>,
    observed: bool,
}

impl ChannelActivityStream {
    /// Read one validated snapshot, without automatic reconnection.
    /// Clean EOF after a snapshot ends this connection normally.
    pub async fn next(&mut self) -> Option<Result<ChannelActivityEvent>> {
        let next = self.stream.as_mut()?.next().await;
        let result = match next {
            Some(Ok(frame)) => (|| {
                let page: ActivityPage = serde_json::from_value(frame.data)
                    .map_err(|_| Error::protocol("invalid Activity snapshot"))?;
                if page.cursor.is_empty()
                    || frame.explicit_id.as_deref() != Some(page.cursor.as_str())
                {
                    return Err(Error::protocol(
                        "Activity snapshot has no matching explicit cursor",
                    ));
                }
                let event = match frame.event.as_str() {
                    "activity" => ChannelActivityEvent::Activity(page),
                    "reset" => ChannelActivityEvent::Reset(page),
                    _ => return Err(Error::protocol("unknown Activity stream event")),
                };
                self.observed = true;
                Ok(event)
            })(),
            Some(Err(error)) => Err(error),
            None => {
                self.stream = None;
                return (!self.observed).then(|| {
                    Err(Error::protocol(
                        "Activity stream closed without an initial snapshot",
                    ))
                });
            }
        };
        if result.is_err() {
            self.stream = None;
        }
        Some(result)
    }
}
