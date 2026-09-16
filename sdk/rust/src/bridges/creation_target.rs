use super::{Bridges, mutations, validation};
use crate::{Error, IdempotencyKey, Result, http::Request, resource_access::ResourceOwner};
use serde::Deserialize;

/// Read-only setup identity. Previewing creates no Bridge or provider objects.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct BridgeCreationTarget {
    /// Identity that creation with the same human and idempotency key will use.
    pub bridge_id: String,
    /// Currently authorized owner scope.
    pub owner: ResourceOwner,
    /// Server-derived callback to configure on the selected provider key.
    pub callback_url: String,
}

impl Bridges<'_> {
    /// Preview the real callback before selecting routes. Preserve this key for
    /// create and retries; changing the key or signed-in human changes the target.
    pub async fn creation_target(
        &self,
        organization: Option<&str>,
        key: &IdempotencyKey,
    ) -> Result<BridgeCreationTarget> {
        mutations::management(key)?;
        if !key.as_str().bytes().all(|byte| byte.is_ascii_graphic()) {
            return Err(Error::validation(
                "bridge creation key must contain visible ASCII without spaces",
            ));
        }
        let mut query = vec![];
        if let Some(org) = organization {
            validation::id(org)?;
            query.push(("organization", org.to_owned()));
        }
        let mut request =
            Request::get(&["bridge-creation-target"]).with_command_key(Some(key.as_str()))?;
        request.query = &query;
        let target: BridgeCreationTarget = self.client.request(request).await?;
        let callback = reqwest::Url::parse(&target.callback_url)
            .map_err(|_| Error::protocol("invalid bridge creation callback URL"))?;
        let scoped = match (&target.owner, organization) {
            (ResourceOwner::Person { .. }, None) => true,
            (ResourceOwner::Organization { subject }, Some(org)) => subject == org,
            _ => false,
        };
        if !validation::valid_id(&target.bridge_id)
            || !validation::owner(&target.owner)
            || !scoped
            || callback.scheme() != "https"
            || callback.host_str().is_none()
            || !callback.username().is_empty()
            || callback.password().is_some()
            || callback.query().is_some()
            || callback.fragment().is_some()
            || callback.path_segments().and_then(Iterator::last) != Some(target.bridge_id.as_str())
        {
            return Err(Error::protocol(
                "bridge creation target does not match its requested scope or callback",
            ));
        }
        Ok(target)
    }
}
