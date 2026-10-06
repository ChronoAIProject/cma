//! Generic gateway-owned connection creation. Platform credentials never enter CMA.
use layer2_core::{bots::BotError, resource_access::ResourceOwner};
use reqwest::{Client, Method, Url};
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Clone)]
pub(crate) struct ChannelConnections {
    http: Client,
    base: Url,
    public: Url,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Connection {
    pub connection_ref: String,
    pub status: String,
    #[serde(default)]
    pub setup_url: Option<String>,
    #[serde(default)]
    pub label: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
}

impl ChannelConnections {
    pub(crate) fn new(http: Client, base: &str, public: &str) -> Result<Self, String> {
        Ok(Self {
            http,
            base: Url::parse(base).map_err(|_| "invalid gateway origin")?,
            public: Url::parse(public).map_err(|_| "invalid gateway public origin")?,
        })
    }

    async fn request(
        &self,
        token: &str,
        owner: &ResourceOwner,
        method: Method,
        id: Option<&str>,
        body: Option<Value>,
        agent_credential: Option<&str>,
    ) -> Result<Value, BotError> {
        let mut url = self.base.clone();
        let mut path = url.path_segments_mut().map_err(|_| BotError::Unavailable)?;
        path.pop_if_empty().extend(["v1", "channel-connections"]);
        if let Some(id) = id {
            reference(id)?;
            path.push(id);
        }
        drop(path);
        let mut request = self.http.request(method, url).bearer_auth(token);
        if let ResourceOwner::Organization { subject } = owner {
            request = request.header("X-CMAEG-Owner", subject.as_str());
        }
        if let Some(credential) = agent_credential {
            request = request.header(
                "X-CMAEG-Agent-Authorization",
                format!("Bearer {credential}"),
            );
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request.send().await.map_err(|_| BotError::Unavailable)?;
        if matches!(response.status().as_u16(), 401 | 403) {
            return Err(BotError::Forbidden);
        }
        if response.status() == 404 {
            return Err(BotError::NotFound);
        }
        if response.status() == 204 {
            return Ok(Value::Null);
        }
        if response.status() == 409 {
            return Err(BotError::Conflict);
        }
        crate::auth::provider_http::read_json(response)
            .await
            .map_err(|_| BotError::Unavailable)
    }

    pub(crate) async fn list(
        &self,
        token: &str,
        owner: &ResourceOwner,
    ) -> Result<Vec<Connection>, BotError> {
        #[derive(Deserialize)]
        struct Listing {
            items: Vec<Connection>,
        }
        let result: Listing = serde_json::from_value(
            self.request(token, owner, Method::GET, None, None, None)
                .await?,
        )
        .map_err(|_| BotError::Unavailable)?;
        if result.items.len() > 1000 {
            return Err(BotError::Unavailable);
        }
        let mut seen = std::collections::BTreeSet::new();
        for item in &result.items {
            self.validate(item)?;
            if !seen.insert(&item.connection_ref) {
                return Err(BotError::Unavailable);
            }
        }
        Ok(result
            .items
            .into_iter()
            .filter(|item| item.status == "ready")
            .collect())
    }

    pub(crate) async fn create(
        &self,
        token: &str,
        owner: &ResourceOwner,
        record_id: &str,
        event_ref: &str,
        agent_credential: &str,
        suggested_name: Option<&str>,
    ) -> Result<Connection, BotError> {
        let id = uuid::Uuid::parse_str(record_id).map_err(|_| BotError::Conflict)?;
        if id.get_version_num() != 4 {
            return Err(BotError::Conflict);
        }
        let value = self.request(token, owner, Method::POST, None,
            Some(json!({"record_id": record_id, "event_ref": event_ref, "suggested_name": suggested_name})),
            Some(agent_credential)).await?;
        let connection = serde_json::from_value(value).map_err(|_| BotError::Unavailable)?;
        self.validate(&connection)?;
        if connection.connection_ref != record_id {
            return Err(BotError::Forbidden);
        }
        Ok(connection)
    }

    pub(crate) async fn get(
        &self,
        token: &str,
        owner: &ResourceOwner,
        id: &str,
    ) -> Result<Connection, BotError> {
        let connection: Connection = serde_json::from_value(
            self.request(token, owner, Method::GET, Some(id), None, None)
                .await?,
        )
        .map_err(|_| BotError::Unavailable)?;
        self.validate(&connection)?;
        if connection.connection_ref != id {
            return Err(BotError::Forbidden);
        }
        Ok(connection)
    }

    pub(crate) async fn cancel(
        &self,
        token: &str,
        owner: &ResourceOwner,
        id: &str,
    ) -> Result<(), BotError> {
        match self
            .request(token, owner, Method::DELETE, Some(id), None, None)
            .await
        {
            Ok(_) | Err(BotError::NotFound) => Ok(()),
            Err(error) => Err(error),
        }
    }

    fn validate(&self, connection: &Connection) -> Result<(), BotError> {
        reference(&connection.connection_ref)?;
        if connection.status.is_empty()
            || connection.status.len() > 64
            || connection
                .label
                .as_ref()
                .is_some_and(|label| label.len() > 256)
        {
            return Err(BotError::Unavailable);
        }
        if let Some(value) = &connection.setup_url {
            let url = Url::parse(value).map_err(|_| BotError::Unavailable)?;
            if url.origin() != self.public.origin()
                || !url.username().is_empty()
                || url.password().is_some()
            {
                return Err(BotError::Forbidden);
            }
        }
        if let Some(value) = &connection.url {
            let url = Url::parse(value).map_err(|_| BotError::Unavailable)?;
            if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
                return Err(BotError::Forbidden);
            }
        }
        Ok(())
    }
}

fn reference(value: &str) -> Result<(), BotError> {
    if value.is_empty()
        || value.len() > 4096
        || matches!(value, "." | "..")
        || value.chars().any(char::is_control)
    {
        Err(BotError::Unavailable)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests;
