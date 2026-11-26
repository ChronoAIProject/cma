//! Service inventory and hosted connections under the user's selected owner.
use super::*;
use crate::auth::{
    keys::{owner_matches, valid_provider_key_id},
    provider_http::read_json,
};
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

#[derive(Debug, Clone, Serialize)]
pub struct ActorServiceChoice {
    pub id: String,
    pub slug: String,
    pub name: String,
    pub connected: bool,
    pub connection_url: Option<String>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceConnection {
    pub id: String,
    #[serde(rename = "connect_url")]
    pub url: String,
    pub expires_at: String,
}
#[derive(Deserialize)]
struct Inventory {
    services: Vec<ConnectedService>,
}
#[derive(Deserialize)]
struct ConnectedService {
    id: String,
    slug: String,
    is_active: bool,
    credential_source: Value,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    label: Option<String>,
}
#[derive(Deserialize)]
struct Catalogue {
    entries: Vec<CatalogueEntry>,
}
#[derive(Deserialize)]
struct CatalogueEntry {
    slug: String,
    name: String,
}

impl AuthState {
    pub async fn services(
        &self,
        grant_id: &str,
        actor: &EventActor,
        owner: &ResourceOwner,
    ) -> Result<Vec<ActorServiceChoice>, BotError> {
        let (_, token) = self.context_for_grant(grant_id, actor, owner).await?;
        let inventory: Inventory = self
            .actor_provider_json(token.expose(), Method::GET, "/api/v1/user-services", None)
            .await?;
        let catalog: Catalogue = self
            .actor_provider_json(
                token.expose(),
                Method::GET,
                "/api/v1/catalog?include_all=true",
                None,
            )
            .await?;
        if inventory.services.len() > 1000 || catalog.entries.len() > 1000 {
            return Err(BotError::Unavailable);
        }
        let mut choices = Vec::new();
        let mut ids = std::collections::HashSet::new();
        for service in inventory
            .services
            .into_iter()
            .filter(|service| owner_matches(&service.credential_source, owner))
        {
            if !valid_provider_key_id(&service.id)
                || !valid_provider_key_id(&service.slug)
                || !ids.insert(service.id.clone())
            {
                return Err(BotError::Unavailable);
            }
            if !service.is_active {
                continue;
            }
            let name = service
                .label
                .or(service.name)
                .or_else(|| {
                    catalog
                        .entries
                        .iter()
                        .find(|entry| entry.slug == service.slug)
                        .map(|entry| entry.name.clone())
                })
                .unwrap_or_else(|| service.slug.clone());
            choices.push(ActorServiceChoice {
                id: service.id,
                slug: service.slug,
                name,
                connected: true,
                connection_url: None,
            });
        }
        for service in catalog.entries {
            if !valid_provider_key_id(&service.slug) {
                return Err(BotError::Unavailable);
            }
            if !choices.iter().any(|choice| choice.slug == service.slug) {
                choices.push(ActorServiceChoice {
                    id: service.slug.clone(),
                    slug: service.slug,
                    name: service.name,
                    connected: false,
                    connection_url: None,
                });
            }
        }
        choices.sort_by(|a, b| a.name.cmp(&b.name).then(a.id.cmp(&b.id)));
        Ok(choices)
    }

    pub async fn start_service_connection(
        &self,
        grant_id: &str,
        actor: &EventActor,
        owner: &ResourceOwner,
        service_id: &str,
    ) -> Result<ServiceConnection, BotError> {
        let choices = self.services(grant_id, actor, owner).await?;
        let choice = choices
            .iter()
            .find(|choice| choice.id == service_id && !choice.connected)
            .ok_or(BotError::Forbidden)?;
        let (_, user_token) = self.context_for_grant(grant_id, actor, owner).await?;
        // NyxID's hosted connection links use the calling subject as owner.
        // For organizations, a five-minute empty-grant key supplies that exact
        // subject. The user still approves credentials in NyxID's hosted UI;
        // completion independently checks current organization administration.
        let ephemeral = match owner {
            ResourceOwner::Organization { subject } => Some(
                self.org_connection_key(user_token.expose(), subject)
                    .await?,
            ),
            ResourceOwner::Person { .. } => None,
        };
        let link_token = ephemeral
            .as_ref()
            .map(|(_, token)| token.expose())
            .unwrap_or(user_token.expose());
        let result: Result<ServiceConnection, BotError> = self
            .actor_provider_json(
                link_token,
                Method::POST,
                "/api/v1/connect-links",
                Some(json!({
                    "service_slug":choice.slug,"requested_by":"CMA Bot Father","expires_in":900
                })),
            )
            .await;
        if let Some((key_id, _)) = ephemeral {
            self.revoke_connection_key(user_token.expose(), &key_id)
                .await?;
        }
        let connection = result?;
        self.validate_connection_url(&connection)?;
        Ok(connection)
    }

    #[allow(dead_code)] // Polling is exposed to the Telegram workflow after hosted consent.
    pub(crate) async fn service_connection_status(
        &self,
        grant_id: &str,
        actor: &EventActor,
        owner: &ResourceOwner,
        connection_id: &str,
    ) -> Result<bool, BotError> {
        if !valid_provider_key_id(connection_id) {
            return Err(BotError::Forbidden);
        }
        let (_, token) = self.context_for_grant(grant_id, actor, owner).await?;
        let result: Value = self
            .actor_provider_json(
                token.expose(),
                Method::GET,
                &format!("/api/v1/connect-links/{connection_id}"),
                None,
            )
            .await?;
        if result.get("id").and_then(Value::as_str) != Some(connection_id) {
            return Err(BotError::Unavailable);
        }
        if result.get("status").and_then(Value::as_str) != Some("completed") {
            return Ok(false);
        }
        let id = result
            .pointer("/connected_service/id")
            .and_then(Value::as_str)
            .ok_or(BotError::Unavailable)?;
        let slug = result
            .pointer("/connected_service/slug")
            .and_then(Value::as_str)
            .ok_or(BotError::Unavailable)?;
        Ok(self
            .services(grant_id, actor, owner)
            .await?
            .iter()
            .any(|service| service.connected && service.id == id && service.slug == slug))
    }

    async fn org_connection_key(
        &self,
        token: &str,
        org: &SubjectId,
    ) -> Result<(String, SecretValue), BotError> {
        if !valid_provider_key_id(org.as_str()) {
            return Err(BotError::Forbidden);
        }
        let name = format!("cma:bot-father:connect:{}", uuid::Uuid::new_v4());
        let expires_at = layer2_core::rfc3339_at(SystemTime::now() + Duration::from_secs(300));
        let mut response: Value = self.actor_provider_json(token, Method::POST, "/api/v1/api-keys", Some(json!({
            "name":name,"scopes":"write","target_org_id":org.as_str(),"expires_at":expires_at,
            "allowed_service_ids":[],"allowed_node_ids":[],"allow_all_services":false,"allow_all_nodes":false,"allow_auto_connected_services":false
        }))).await?;
        let id = response
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| valid_provider_key_id(id))
            .ok_or(BotError::Unavailable)?
            .to_string();
        let key = response
            .get_mut("full_key")
            .map(Value::take)
            .and_then(|value| value.as_str().map(str::to_owned))
            .map(SecretValue::new)
            .ok_or(BotError::Unavailable)?;
        let valid = response.get("name").and_then(Value::as_str) == Some(&name)
            && response.get("scopes").and_then(Value::as_str) == Some("write")
            && closed_empty_grant(&response)
            && crate::auth::keys::KeyIssuer::provided(key.expose()).is_ok();
        if !valid {
            self.revoke_connection_key(token, &id).await?;
            return Err(BotError::Forbidden);
        }
        let observed: Result<Value, BotError> = self
            .actor_provider_json(token, Method::GET, &format!("/api/v1/api-keys/{id}"), None)
            .await;
        let owned = observed.as_ref().is_ok_and(|value| {
            value.get("id").and_then(Value::as_str) == Some(id.as_str())
                && value.get("name").and_then(Value::as_str) == Some(name.as_str())
                && owner_matches(
                    &value["credential_source"],
                    &ResourceOwner::Organization {
                        subject: org.clone(),
                    },
                )
                && value.get("scopes").and_then(Value::as_str) == Some("write")
                && closed_empty_grant(value)
                && value
                    .get("expires_at")
                    .and_then(Value::as_str)
                    .and_then(layer2_core::seconds_from_rfc3339)
                    == layer2_core::seconds_from_rfc3339(&expires_at)
        });
        if !owned {
            self.revoke_connection_key(token, &id).await?;
            return Err(BotError::Forbidden);
        }
        Ok((id, key))
    }
    async fn revoke_connection_key(&self, token: &str, id: &str) -> Result<(), BotError> {
        let AuthMode::Provider { base_url, .. } = &self.mode else {
            return Err(BotError::Unavailable);
        };
        let response = self
            .http
            .delete(format!(
                "{}/api/v1/api-keys/{id}",
                base_url.trim_end_matches('/')
            ))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| BotError::Unavailable)?;
        if response.status().is_success() || response.status() == 404 {
            Ok(())
        } else {
            Err(BotError::Unavailable)
        }
    }
    fn validate_connection_url(&self, connection: &ServiceConnection) -> Result<(), BotError> {
        let url = reqwest::Url::parse(&connection.url).map_err(|_| BotError::Unavailable)?;
        let origin =
            reqwest::Url::parse(&self.nyxid_public_url).map_err(|_| BotError::Unavailable)?;
        let valid_path = url
            .path()
            .strip_prefix("/connect/nyx_clk_")
            .is_some_and(|value| {
                value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
            });
        if url.origin() != origin.origin()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !valid_path
            || !valid_provider_key_id(&connection.id)
        {
            return Err(BotError::Unavailable);
        }
        Ok(())
    }
    pub(crate) async fn actor_provider_json<T: DeserializeOwned>(
        &self,
        token: &str,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<T, BotError> {
        let AuthMode::Provider { base_url, .. } = &self.mode else {
            return Err(BotError::Unavailable);
        };
        let request = self
            .http
            .request(method, format!("{}{path}", base_url.trim_end_matches('/')))
            .bearer_auth(token);
        let request = if let Some(body) = body {
            request.json(&body)
        } else {
            request
        };
        read_json(request.send().await.map_err(|_| BotError::Unavailable)?)
            .await
            .map_err(|_| BotError::Unavailable)
    }
}
fn closed_empty_grant(value: &Value) -> bool {
    [
        "allow_all_services",
        "allow_all_nodes",
        "allow_auto_connected_services",
    ]
    .iter()
    .all(|key| value.get(key).and_then(Value::as_bool) == Some(false))
        && ["allowed_service_ids", "allowed_node_ids"]
            .iter()
            .all(|key| {
                value
                    .get(key)
                    .and_then(Value::as_array)
                    .is_some_and(Vec::is_empty)
            })
}
