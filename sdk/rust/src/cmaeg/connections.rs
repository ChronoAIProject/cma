use super::{Cmaeg, types::*};
use crate::{Error, Result, http::Request};
use reqwest::{Method, StatusCode, header::HeaderValue};

/// Explicit owner selection for one connection operation.
#[derive(Default)]
pub struct ConnectionOptions<'a> {
    /// Organization subject; omitted selects the authenticated creator.
    pub owner: Option<&'a str>,
}

fn apply_owner(request: &mut Request<'_>, options: &ConnectionOptions<'_>) -> Result<()> {
    request.root = true;
    if let Some(owner) = options.owner {
        if owner.is_empty() || owner.len() > 512 || !owner.bytes().all(|b| (33..=126).contains(&b))
        {
            return Err(Error::validation("invalid connection owner"));
        }
        request.headers.insert(
            "x-cmaeg-owner",
            HeaderValue::from_str(owner)
                .map_err(|_| Error::validation("invalid connection owner"))?,
        );
    }
    Ok(())
}

impl Cmaeg {
    /// List this credential subject's eligible Lark/Feishu bot API connections.
    /// Does not select another owner, create credentials, or grant channel access.
    pub async fn list_relay_resource_connections(
        &self,
        platform: ResourcePlatform,
    ) -> Result<ResourceConnectionList> {
        let platform = match platform {
            ResourcePlatform::Lark => "lark",
            ResourcePlatform::Feishu => "feishu",
        };
        let query = [("platform", platform.to_owned())];
        let mut request = Request::get(&["v1", "relay-resource-connections"]);
        request.root = true;
        request.query = &query;
        self.client.request(request).await
    }

    /// Read the gateway's platform connection capabilities.
    pub async fn get_channel_capabilities(&self) -> Result<CapabilityView> {
        let mut request = Request::get(&["v1", "channel-capabilities"]);
        request.root = true;
        self.client.request(request).await
    }

    /// Reserve an opaque connection with separate creator and originating-agent authority.
    pub async fn create_channel_connection(
        &self,
        body: &CreateConnection,
        agent_key: &str,
        options: &ConnectionOptions<'_>,
    ) -> Result<ConnectionView> {
        if agent_key.is_empty()
            || agent_key.len() > 16384
            || !agent_key.bytes().all(|b| (33..=126).contains(&b))
        {
            return Err(Error::validation("invalid originating agent credential"));
        }
        let mut request = Request::operation(
            Method::POST,
            &["v1", "channel-connections"],
            Some(body),
            StatusCode::CREATED,
        )?;
        apply_owner(&mut request, options)?;
        let mut value = HeaderValue::from_str(&format!("Bearer {agent_key}"))
            .map_err(|_| Error::validation("invalid originating agent credential"))?;
        value.set_sensitive(true);
        request.headers.insert("x-cmaeg-agent-authorization", value);
        self.client.request(request).await
    }

    /// Read one page; an empty page may still contain a next cursor.
    pub async fn list_channel_connections(
        &self,
        after: Option<&str>,
        options: &ConnectionOptions<'_>,
    ) -> Result<ConnectionPage> {
        let query: Vec<_> = after
            .map(|value| ("after", value.to_owned()))
            .into_iter()
            .collect();
        let mut request = Request::get(&["v1", "channel-connections"]);
        apply_owner(&mut request, options)?;
        request.query = &query;
        self.client.request(request).await
    }

    /// Read an owner-visible connection without interpreting its opaque reference.
    pub async fn get_channel_connection(
        &self,
        reference: &str,
        options: &ConnectionOptions<'_>,
    ) -> Result<ConnectionView> {
        let segments = ["v1", "channel-connections", reference];
        let mut request = Request::get(&segments);
        apply_owner(&mut request, options)?;
        self.client.request(request).await
    }

    /// Cancel one connection without retrying an uncertain mutation.
    pub async fn cancel_channel_connection(
        &self,
        reference: &str,
        options: &ConnectionOptions<'_>,
    ) -> Result<()> {
        let segments = ["v1", "channel-connections", reference];
        let mut request = Request::operation(
            Method::DELETE,
            &segments,
            None::<&()>,
            StatusCode::NO_CONTENT,
        )?;
        apply_owner(&mut request, options)?;
        self.client.empty(request).await
    }
}
