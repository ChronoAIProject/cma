//! Bounded read-only owner inventory used by CMA bridge setup.
use super::*;
use layer2_core::bridge::setup::{
    BridgeBotChoice, BridgeConnectorChoices, BridgeKeyChoice, BridgeRouteChoice,
};

impl NyxBridgeProvider {
    pub(crate) async fn choices(
        &self,
        owner: &ResourceOwner,
        subject: &SubjectId,
        credential: &SecretValue,
    ) -> Result<BridgeConnectorChoices, BridgeError> {
        if !canonical_id(owner_subject(owner).as_str())
            || !canonical_id(subject.as_str())
            || matches!(owner, ResourceOwner::Person { subject: owner } if owner != subject)
        {
            return Err(binding_mismatch());
        }
        tokio::time::timeout(DEADLINE, async {
            let (bots, keys, routes) = tokio::try_join!(
                self.get::<Bots>("/api/v1/channel-bots", owner, credential),
                self.get::<Keys>("/api/v1/api-keys", owner, credential),
                self.get::<Routes>("/api/v1/channel-conversations", owner, credential),
            )?;
            require_complete(
                bots.bots.len(),
                Some(bots.total),
                bots.bots.iter().map(|b| b.id.as_str()),
            )?;
            require_complete(
                keys.keys.len(),
                None,
                keys.keys.iter().map(|k| k.id.as_str()),
            )?;
            require_complete(
                routes.conversations.len(),
                Some(routes.total),
                routes.conversations.iter().map(|r| r.id.as_str()),
            )?;
            if bots
                .bots
                .iter()
                .any(|b| b.user_id != owner_subject(owner).as_str())
            {
                return Err(binding_mismatch());
            }
            for key in &keys.keys {
                key.credential_source.require_owner(owner, subject)?;
            }
            Ok(BridgeConnectorChoices {
                bots: bots
                    .bots
                    .into_iter()
                    .filter_map(|b| {
                        Some(BridgeBotChoice {
                            platform: super::super::capability_catalog::supported_platform(
                                &b.platform,
                            )?,
                            id: b.id,
                            active: b.is_active,
                        })
                    })
                    .collect(),
                keys: keys
                    .keys
                    .into_iter()
                    .map(|k| BridgeKeyChoice {
                        id: k.id,
                        active: k.is_active,
                        callback_configured: k.callback_url.is_some(),
                    })
                    .collect(),
                routes: routes
                    .conversations
                    .into_iter()
                    .map(|r| BridgeRouteChoice {
                        id: r.id,
                        bot_id: r.channel_bot_id,
                        key_id: r.agent_api_key_id,
                        platform: r.platform,
                        active: r.is_active,
                    })
                    .collect(),
            })
        })
        .await
        .map_err(|_| BridgeError::AuthorityUnavailable)?
    }
}
