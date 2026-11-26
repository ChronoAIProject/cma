//! Resolve and admit immutable catalogue or inline Bot profiles.
use super::*;
use crate::profiles::{AgentProfileRef, SandboxProfileRef, WorkspaceProfileRef};

impl BotService {
    pub(super) async fn create_owned(
        &self,
        owner: &SubjectId,
        authority: Option<&BotOwnerAuthority>,
        delegated_credential: Option<&str>,
        key: &str,
        mut request: CreateBot,
    ) -> Result<Bot, BotError> {
        request.validate()?;
        if key.is_empty() || key.len() > 256 || key.chars().any(char::is_control) {
            return Err(BotError::Invalid("an idempotency key is required".into()));
        }
        let key_hash = if let Some(authority) = authority {
            hash(
                &serde_json::to_vec(&("owned-bot-create-v1", authority.owner(), key))
                    .map_err(|_| BotError::Unavailable)?,
            )
        } else {
            hash(key.as_bytes())
        };
        let fingerprint = crate::profiles::content_digest(&request)
            .map_err(|_| BotError::Invalid("invalid bot request".into()))?;
        if let Some(current) = self.store.creation(owner, &key_hash).await? {
            return replay_owned(current, &fingerprint, authority);
        }
        let profiles = if request.inline_agent.is_some() {
            self.inline_profiles
                .as_ref()
                .ok_or(BotError::Unavailable)?
                .resolve_inline(owner, &request, delegated_credential, key)
                .await?
        } else if let Some(authority) = authority {
            self.profiles
                .resolve_for_owner(authority.owner(), &request)
                .await?
        } else {
            self.profiles.resolve(owner, &request).await?
        };
        if request.inline_agent.is_some() {
            request.profile = AgentProfileRef {
                id: profiles.agent.profile_id.clone(),
                revision: profiles.agent.revision,
            };
            request.sandbox_profile = Some(SandboxProfileRef::new(
                profiles.sandbox.profile_id.clone(),
                profiles.sandbox.revision,
            ));
            request.workspace_profile = Some(WorkspaceProfileRef::new(
                profiles.workspace.profile_id.clone(),
                profiles.workspace.revision,
            ));
        }
        if let Some(authority) = authority {
            authority.require(Capability::ManageOwnBots)?;
        }
        let now = crate::now_rfc3339();
        let bot = Bot {
            ownership: authority.map(|authority| authority.ownership().clone()),
            bot_id: format!("bot_{}", uuid::Uuid::new_v4().simple()),
            owner: owner.clone(),
            request,
            profiles,
            provider: self.provider.clone(),
            record_id: uuid::Uuid::new_v4().to_string(),
            resources: BotResources::default(),
            state: BotState::Creating,
            work: Some(work(BotOperation::Create, 1, self.effect_reservations)),
            generation: 1,
            revision: 1,
            creation_key_hash: key_hash.clone(),
            request_fingerprint: fingerprint.clone(),
            created_at: now.clone(),
            updated_at: now,
            last_test: None,
        };
        bot.validate()?;
        let result = if bot.request.inline_agent.is_some() {
            self.store.insert(&bot).await
        } else {
            self.store.insert_catalogue(&bot).await
        };
        match result {
            Ok(()) => Ok(bot),
            Err(BotError::Conflict) => replay_owned(
                self.store
                    .creation(owner, &key_hash)
                    .await?
                    .ok_or(BotError::Conflict)?,
                &fingerprint,
                authority,
            ),
            Err(error) => Err(error),
        }
    }
}
