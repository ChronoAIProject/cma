use super::*;

impl KeyIssuer {
    /// Exact Bot namespace and optional relay callback; an empty grant is only
    /// permitted for a route key with every broad grant explicitly disabled.
    #[cfg(test)]
    pub(crate) async fn mint_bot(
        &self,
        name: &str,
        token: &str,
        services: &[ServiceId],
        callback: Option<&str>,
        owner: &layer2_core::resource_access::ResourceOwner,
    ) -> Result<MintedKey, KeyIssuerError> {
        self.mint_bot_admitted(name, token, services, callback, owner, || true)
            .await
    }

    /// Mint a Bot key, invoking the caller's reservation fence immediately
    /// before the consuming key POST after all authority reads complete.
    pub(crate) async fn mint_bot_admitted<F: Fn() -> bool>(
        &self,
        name: &str,
        token: &str,
        services: &[ServiceId],
        callback: Option<&str>,
        owner: &layer2_core::resource_access::ResourceOwner,
        check: F,
    ) -> Result<MintedKey, KeyIssuerError> {
        let allowed = if callback.is_some() && services.is_empty() {
            Vec::new()
        } else {
            self.resolve_bot_services(token, services, owner).await?
        };
        let plan = self.scope_plan(token, &allowed, owner).await?;
        let expires_at = key_expiry(SystemTime::now());
        let mut body = serde_json::json!({
            "name":name,"scopes":KEY_SCOPES,"allowed_service_ids":allowed,
            "allowed_node_ids":[],"allow_all_services":false,"allow_all_nodes":false,"allow_auto_connected_services":false,
            "expires_at":layer2_core::rfc3339_at(expires_at)
        });
        plan.apply(&mut body);
        if let layer2_core::resource_access::ResourceOwner::Organization { subject } = owner {
            body["target_org_id"] = subject.as_str().into();
        }
        if let Some(callback) = callback {
            body["callback_url"] = callback.into();
            // NyxID API-key platform describes the client, not the channel.
            body["platform"] = "generic".into();
        }
        if !check() {
            return Err(KeyIssuerError::Unavailable(
                "bot key admission deadline elapsed".into(),
            ));
        }
        let response = self
            .http
            .post(format!("{}{KEYS_PATH}", self.base_url))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await
            .map_err(|_| KeyIssuerError::Unavailable("bot key creation unavailable".into()))?;
        let response = super::super::provider_http::read_json(response)
            .await
            .map_err(|_| KeyIssuerError::UnexpectedAuthority)?;
        minted_key_from_response(response, &allowed, &plan.allowed_node_ids, expires_at)
            .ok_or(KeyIssuerError::UnexpectedAuthority)
    }

    async fn resolve_bot_services(
        &self,
        token: &str,
        services: &[ServiceId],
        owner: &layer2_core::resource_access::ResourceOwner,
    ) -> Result<Vec<String>, KeyIssuerError> {
        if services.is_empty() {
            return Err(KeyIssuerError::EmptyGrant);
        }
        let response = self
            .http
            .get(format!("{}{USER_SERVICES_PATH}", self.base_url))
            .bearer_auth(token)
            .send()
            .await
            .map_err(|_| {
                KeyIssuerError::Unavailable("bot service connections unavailable".into())
            })?;
        let listing: BotServices = super::super::provider_http::read_json(response)
            .await
            .map_err(|_| KeyIssuerError::UnexpectedAuthority)?;
        let (rows, total) = match listing {
            BotServices::Bare(rows) => (rows, None),
            BotServices::Wrapped { services, total } => (services, total),
        };
        if rows.len() > 1000 || total.is_some_and(|t| t != rows.len()) {
            return Err(KeyIssuerError::UnexpectedAuthority);
        }
        let mut granted = Vec::new();
        for service in services {
            let matches: Vec<_> = rows
                .iter()
                .filter(|row| owner_matches(&row.credential_source, owner))
                .filter(|row| {
                    row.id == service.as_str() || row.slug.as_deref() == Some(service.as_str())
                })
                .collect();
            let [row] = matches.as_slice() else {
                return Err(KeyIssuerError::ServiceNotGranted(service.clone()));
            };
            if !row.is_active || !valid_provider_key_id(&row.id) {
                return Err(KeyIssuerError::ServiceNotGranted(service.clone()));
            }
            if !granted.contains(&row.id) {
                granted.push(row.id.clone());
            }
        }
        Ok(granted)
    }
}

pub(crate) fn owner_matches(
    source: &serde_json::Value,
    owner: &layer2_core::resource_access::ResourceOwner,
) -> bool {
    use layer2_core::resource_access::ResourceOwner;
    match owner {
        ResourceOwner::Person { .. } => {
            source.get("type").and_then(serde_json::Value::as_str) == Some("personal")
        }
        ResourceOwner::Organization { subject } => {
            source.get("type").and_then(serde_json::Value::as_str) == Some("org")
                && source.get("org_id").and_then(serde_json::Value::as_str)
                    == Some(subject.as_str())
                && source.get("allowed").and_then(serde_json::Value::as_bool) != Some(false)
        }
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum BotServices {
    Bare(Vec<BotService>),
    Wrapped {
        #[serde(alias = "items", alias = "user_services")]
        services: Vec<BotService>,
        total: Option<usize>,
    },
}
#[derive(Deserialize)]
struct BotService {
    id: String,
    slug: Option<String>,
    is_active: bool,
    credential_source: serde_json::Value,
}

#[cfg(test)]
mod tests;
