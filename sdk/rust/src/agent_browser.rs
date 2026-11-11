#![doc = "Explicit Agent browser enablement and bounded authorized observations."]
use crate::{
    Client, Error, Result,
    http::{Request, receive_bounded},
};
use reqwest::{Method, StatusCode};
use serde::Deserialize;

#[doc = "One bounded captured image, never an input or execution channel."]
#[derive(Clone, Deserialize, PartialEq)]
pub struct BrowserFrame {
    #[doc = "Monotonic sequence within the browser context."]
    pub sequence: u64,
    #[doc = "Actual capture timestamp in RFC3339."]
    pub captured_at: String,
    #[doc = "Image media type, currently image/jpeg."]
    pub media_type: String,
    #[doc = "Captured width."]
    pub width: u32,
    #[doc = "Captured height."]
    pub height: u32,
    #[doc = "Bounded base64 image bytes."]
    pub data: String,
}

impl std::fmt::Debug for BrowserFrame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("BrowserFrame")
            .field("sequence", &self.sequence)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("encoded_bytes", &self.data.len())
            .finish_non_exhaustive()
    }
}

#[doc = "A current observation scoped to an Agent, runtime and browser context."]
#[derive(Clone, Debug, Deserialize, PartialEq)]
pub struct BrowserObservation {
    #[doc = "Requested Agent identity."]
    pub agent_id: String,
    #[doc = "Opaque current runtime identity."]
    pub runtime_incarnation: String,
    #[doc = "Opaque browser context, absent before enablement."]
    pub context_id: Option<String>,
    #[doc = "not_enabled, unsupported, loading, ready, live, paused or disconnected."]
    pub state: String,
    #[doc = "Sanitized location without addresses or credentials."]
    pub location: String,
    /// Sanitized current origin, when reported by the runtime.
    pub address: Option<String>,
    #[doc = "Actual bounded image, if available."]
    pub frame: Option<BrowserFrame>,
    #[doc = "Current unexpired observer leases."]
    pub observers: u32,
    #[doc = "Total real captures in this mediator incarnation."]
    pub captures: u64,
    #[doc = "Total captured image bytes."]
    pub bytes: u64,
    #[doc = "Minimum capture interval, 1000 milliseconds."]
    pub min_interval_ms: u32,
    #[doc = "Frame freshness threshold, 3000 milliseconds."]
    pub fresh_for_ms: u32,
    #[doc = "Maximum decoded image size, 262144 bytes."]
    pub max_frame_bytes: u32,
    #[doc = "Maximum observer leases, 16."]
    pub max_observers: u32,
}

#[doc = "Browser APIs under current authorization; no retained access grant."]
pub struct AgentBrowser<'a> {
    client: &'a Client,
}

impl Client {
    #[doc = "Observe a browser or explicitly enable it as its owner."]
    pub fn agent_browser(&self) -> AgentBrowser<'_> {
        AgentBrowser { client: self }
    }
}

impl AgentBrowser<'_> {
    #[doc = "Read status, or capture a frame when an observer identifier is supplied."]
    pub async fn read(
        &self,
        agent: &str,
        incarnation: &str,
        observer: Option<&str>,
        context: Option<&str>,
    ) -> Result<BrowserObservation> {
        self.receive(agent, incarnation, observer, context, Method::GET, None)
            .await
    }

    #[doc = "Explicitly enable or disable this Agent browser in the named runtime."]
    pub async fn configure(
        &self,
        agent: &str,
        incarnation: &str,
        enabled: bool,
    ) -> Result<BrowserObservation> {
        self.receive(agent, incarnation, None, None, Method::PUT, Some(enabled))
            .await
    }

    #[doc = "Release observation without navigating or closing the Agent browser."]
    pub async fn release(
        &self,
        agent: &str,
        incarnation: &str,
        observer: &str,
    ) -> Result<BrowserObservation> {
        self.receive(
            agent,
            incarnation,
            Some(observer),
            None,
            Method::DELETE,
            None,
        )
        .await
    }

    async fn receive(
        &self,
        agent: &str,
        incarnation: &str,
        observer: Option<&str>,
        context: Option<&str>,
        method: Method,
        enabled: Option<bool>,
    ) -> Result<BrowserObservation> {
        let segments = ["agents", agent, "browser"];
        let body = enabled.map(
            |enabled| serde_json::json!({"runtime_incarnation":incarnation,"enabled":enabled}),
        );
        let mut query = vec![("runtime_incarnation", incarnation.to_string())];
        if let Some(observer) = observer {
            query.push(("observer", observer.into()));
        }
        if let Some(context) = context {
            query.push(("context_id", context.into()));
        }
        let mut request = Request::operation(method, &segments, body.as_ref(), StatusCode::OK)?;
        if enabled.is_none() {
            request.query = &query;
        }
        let bytes = receive_bounded(&self.client.inner, request, 360000).await?;
        let value: BrowserObservation = serde_json::from_slice(&bytes.body)
            .map_err(|_| Error::protocol("Invalid browser observation"))?;
        use base64::Engine as _;
        let empty_state = value.frame.is_none()
            && matches!(
                value.state.as_str(),
                "paused" | "unsupported" | "not_enabled"
            );
        if value.address.as_ref().is_some_and(|address| {
            address != "about:blank"
                && (address.len() > 8192
                    || !reqwest::Url::parse(address).is_ok_and(|url| {
                        matches!(url.scheme(), "http" | "https")
                            && url.origin().ascii_serialization() == *address
                    }))
        }) || value.agent_id != agent
            || value.runtime_incarnation != incarnation
            || context
                .is_some_and(|context| value.context_id.as_deref() != Some(context) && !empty_state)
            || !matches!(
                value.state.as_str(),
                "not_enabled"
                    | "unsupported"
                    | "loading"
                    | "ready"
                    | "live"
                    | "paused"
                    | "disconnected"
            )
            || !matches!(
                value.location.as_str(),
                "Blank page" | "Local page" | "Web page"
            )
            || value.min_interval_ms != 1000
            || value.fresh_for_ms != 3000
            || value.max_frame_bytes != 262144
            || value.max_observers != 16
            || value.observers > 16
            || (value.state == "live" && (value.frame.is_none() || value.context_id.is_none()))
            || value.frame.as_ref().is_some_and(|frame| {
                frame.sequence == 0
                    || frame.width != 1280
                    || frame.height != 720
                    || frame.media_type != "image/jpeg"
                    || frame.data.len() > 349528
                    || !base64::engine::general_purpose::STANDARD
                        .decode(&frame.data)
                        .is_ok_and(|bytes| {
                            bytes.len() <= 262144 && bytes.starts_with(&[255, 216, 255])
                        })
            })
        {
            return Err(Error::protocol(
                "Browser observation identity or bounds changed",
            ));
        }
        Ok(value)
    }
}
