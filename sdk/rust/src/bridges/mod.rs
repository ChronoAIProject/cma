//! Current bridge setup and management. Callback and reply credentials are not
//! dashboard authentication, and setup receipts never grant deferred execution.
mod capabilities;
mod choices;
mod cleanup;
mod config;
mod invoke;
mod mutations;
mod outbox;
mod source_controls;
mod types;
mod validation;
mod workspace_cleanup;
pub use capabilities::*;
pub use choices::*;
pub use cleanup::*;
pub use config::*;
pub use invoke::*;
pub use outbox::*;
pub use source_controls::*;
pub use types::*;
pub use workspace_cleanup::*;

use crate::{Client, Error, PageOptions, Result, resources::identity};

/// Owner-scoped inventory with an exclusive bridge-identity cursor.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BridgeListOptions {
    /// Organization scope; omission selects the authenticated person.
    pub organization: Option<String>,
    /// Cursor and limit from one through 100; default limit is 50.
    pub page: PageOptions,
}

/// Borrowed bridge operations. This handle retains no selected bot key.
#[derive(Debug)]
pub struct Bridges<'a> {
    client: &'a Client,
}
impl Client {
    /// Manage current bridge definitions and selected existing connector objects.
    pub fn bridges(&self) -> Bridges<'_> {
        Bridges { client: self }
    }
}
impl Bridges<'_> {
    /// List bridges visible under the requested current owner scope.
    pub async fn list(&self, options: &BridgeListOptions) -> Result<BridgePage> {
        if options.page.limit.is_some_and(|n| n == 0 || n > 100) {
            return Err(Error::validation("bridge page limit must be 1 through 100"));
        }
        let mut query = options.page.query();
        if let Some(cursor) = &options.page.cursor {
            validation::id(cursor)?;
        }
        if let Some(org) = &options.organization {
            validation::id(org)?;
            query.push(("organization", org.clone()));
        }
        let page: BridgePage = self.client.get(&["bridges"], &query).await?;
        if page.items.len() > usize::from(options.page.limit.unwrap_or(50)) {
            return Err(Error::protocol("bridge page exceeded its requested limit"));
        }
        let mut previous = options.page.cursor.as_deref();
        for item in &page.items {
            item.check()?;
            if previous.is_some_and(|id| id >= item.id.as_str()) {
                return Err(Error::protocol(
                    "bridge page did not advance strictly after its cursor",
                ));
            }
            if let Some(org) = &options.organization
                && item.owner
                    != (crate::resource_access::ResourceOwner::Organization {
                        subject: org.clone(),
                    })
            {
                return Err(Error::protocol(
                    "bridge page belongs to another organization",
                ));
            }
            previous = Some(&item.id);
        }
        if page
            .next_cursor
            .as_deref()
            .is_some_and(|next| Some(next) != previous || page.items.is_empty())
        {
            return Err(Error::protocol(
                "bridge continuation does not match its page",
            ));
        }
        Ok(page)
    }
    /// Read a safe member/owner projection under current authority.
    pub async fn read(&self, id: &str) -> Result<Bridge> {
        validation::id(id)?;
        let value: Bridge = self.client.get(&["bridges", id], &[]).await?;
        value.check()?;
        identity(&value.id, id)?;
        Ok(value)
    }
    /// Inspect existing provider choices without creating a bot/key or modifying a route.
    pub async fn choices(&self, organization: Option<&str>) -> Result<BridgeConnectorChoices> {
        let mut query = vec![];
        if let Some(org) = organization {
            validation::id(org)?;
            query.push(("organization", org.to_owned()));
        }
        let value: BridgeConnectorChoices = self.client.get(&["bridge-connectors"], &query).await?;
        if value.bots.iter().any(|bot| !validation::valid_id(&bot.id))
            || value.keys.iter().any(|key| !validation::valid_id(&key.id))
            || value.routes.iter().any(|route| {
                !validation::valid_id(&route.id)
                    || !validation::valid_id(&route.key_id)
                    || route.platform.is_empty()
                    || route
                        .bot_id
                        .as_ref()
                        .is_some_and(|id| !validation::valid_id(id))
            })
        {
            return Err(Error::protocol(
                "bridge connector choices have invalid identities",
            ));
        }
        Ok(value)
    }
    /// Read management configuration, preserving older retained partition/placement syntax.
    pub async fn configuration(&self, id: &str) -> Result<BridgeConfigurationDetail> {
        validation::id(id)?;
        let value: BridgeConfigurationDetail = self
            .client
            .get(&["bridges", id, "configuration"], &[])
            .await?;
        value.bridge.check()?;
        identity(&value.bridge.id, id)?;
        value.config.check(false).map_err(Error::protocol)?;
        value.credential.check()?;
        let callback = reqwest::Url::parse(&value.callback_url)
            .map_err(|_| Error::protocol("invalid server-derived bridge callback URL"))?;
        if !value.bridge.can_manage
            || value.config.binding.bot_id != value.bridge.bot_id
            || value.config.binding.platform != value.bridge.platform
            || callback.scheme() != "https"
            || callback.host_str().is_none()
            || !callback.username().is_empty()
            || callback.password().is_some()
            || callback.query().is_some()
            || callback.fragment().is_some()
        {
            return Err(Error::protocol(
                "bridge configuration does not match its authorized projection",
            ));
        }
        Ok(value)
    }
}
