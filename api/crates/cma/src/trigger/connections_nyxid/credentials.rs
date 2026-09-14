//! Consult the credential's connection state, not only its routing row.

use super::{
    decode, malformed, Deserialize, NyxidServiceConnectionBroker, SecretValue,
    ServiceConnectionError, ServiceRequirementState, UserServiceCredentialSource,
};

#[derive(Deserialize)]
struct Connection {
    id: String,
    slug: String,
    is_active: bool,
    status: String,
    connected: bool,
    #[serde(default)]
    connection_status: Option<String>,
    #[serde(default)]
    credential_source: Option<UserServiceCredentialSource>,
    #[serde(default)]
    node_id: Option<String>,
    #[serde(default)]
    node_status: Option<String>,
    credential_missing: bool,
    #[serde(default)]
    credential_type: Option<String>,
}

impl NyxidServiceConnectionBroker {
    pub(super) async fn credential_requirement(
        &self,
        credential: &SecretValue,
        id: &str,
        slug: &str,
    ) -> Result<ServiceRequirementState, ServiceConnectionError> {
        let mut url =
            reqwest::Url::parse(&self.endpoint("/api/v1/keys/")).map_err(|_| malformed())?;
        url.path_segments_mut()
            .map_err(|_| malformed())?
            .pop_if_empty()
            .push(id);
        let response = match self
            .send(self.http.get(url).bearer_auth(credential.expose()), true)
            .await
        {
            Ok(response) => response,
            Err(ServiceConnectionError::NotFound) => return Ok(ServiceRequirementState::Missing),
            Err(error) => return Err(error),
        };
        let connection: Connection = decode(response).await?;
        if connection.id != id || connection.slug != slug {
            return Err(malformed());
        }
        if matches!(
            connection.credential_source,
            Some(UserServiceCredentialSource::Org { allowed: false })
        ) {
            return Ok(ServiceRequirementState::Unknown);
        }
        if !connection.is_active {
            return Ok(ServiceRequirementState::Inactive);
        }
        if connection.credential_missing
            || !connection.connected
            || matches!(
                connection.status.as_str(),
                "pending_auth" | "expired" | "revoked" | "failed" | "refresh_failed"
            )
            || matches!(
                connection.connection_status.as_deref(),
                Some("expired" | "revoked" | "pending_auth")
            )
        {
            return Ok(ServiceRequirementState::Missing);
        }
        if connection.status != "active" {
            return Ok(ServiceRequirementState::Unknown);
        }
        if connection
            .node_id
            .as_deref()
            .is_some_and(|id| !id.is_empty())
        {
            return Ok(match connection.node_status.as_deref() {
                Some("online") => ServiceRequirementState::Satisfied,
                Some("offline" | "stale" | "disconnected" | "draining") => {
                    ServiceRequirementState::Inactive
                }
                _ => ServiceRequirementState::Unknown,
            });
        }
        if connection.credential_type.as_deref() == Some("node_managed") {
            return Ok(ServiceRequirementState::Missing);
        }
        // NyxID's credential_missing flag detects a deleted credential row,
        // not an existing row with no credential material. Its GitHub readiness
        // projection additionally verifies material, without reading secrets or
        // requiring the separate NyxID assistant's tool grants.
        if slug == "api-github" {
            let response = self
                .send(
                    self.http
                        .get(self.endpoint("/api/v1/assistant/readiness"))
                        .bearer_auth(credential.expose()),
                    false,
                )
                .await?;
            let readiness: Readiness = decode(response).await?;
            if readiness.revision != "nyxid-assistant-readiness.v1" {
                return Err(malformed());
            }
            let mut matches = readiness
                .capabilities
                .iter()
                .filter(|capability| capability.capability_id == slug);
            let capability = matches.next().ok_or_else(malformed)?;
            if matches.next().is_some() {
                return Err(malformed());
            }
            return Ok(match capability.connection_state.as_str() {
                "connected" => ServiceRequirementState::Satisfied,
                "connecting" | "disconnected" | "expired" | "revoked" | "verifying" => {
                    ServiceRequirementState::Missing
                }
                _ => ServiceRequirementState::Unknown,
            });
        }
        Ok(ServiceRequirementState::Satisfied)
    }
}

#[derive(Deserialize)]
struct Readiness {
    revision: String,
    capabilities: Vec<Capability>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Capability {
    capability_id: String,
    connection_state: String,
}
