use super::*;
use layer2_core::{
    resource_access::{OrganizationAuthority, OrganizationRole, ResourceOwner},
    Caller, SubjectId,
};

pub(super) fn actor(value: &ExternalActor) -> crate::auth::actor_links::EventActor {
    crate::auth::actor_links::EventActor {
        binding_id: value.binding_id.clone(),
        channel_id: value.channel_id.clone(),
        conversation_id: value.conversation_id.clone(),
        actor_id: value.actor_id.clone(),
        platform: value.platform.clone(),
    }
}
impl SetupDependencies {
    pub(super) async fn confirm_link(
        &self,
        value: &ExternalActor,
        proof: &str,
    ) -> Result<SetupIdentity, BotError> {
        let actor = actor(value);
        let status = self.auth.ensure_actor_link(&actor).await?;
        let candidate = status.candidate.ok_or(BotError::Forbidden)?;
        self.auth
            .confirm_actor_link(&actor, status.revision, &candidate.subject, proof)
            .await?;
        self.link_identity(value).await
    }
    pub(super) async fn owner_choices(
        &self,
        value: &ExternalActor,
        grant: &str,
    ) -> Result<Vec<SetupOwnerChoice>, BotError> {
        let status = self.link_identity(value).await?;
        if status.grant_id.as_deref() != Some(grant) {
            return Err(BotError::Forbidden);
        }
        let subject = SubjectId::new(status.subject.ok_or(BotError::Forbidden)?);
        let personal = ResourceOwner::Person {
            subject: subject.clone(),
        };
        let (context, token) = self
            .auth
            .context_for_grant(grant, &actor(value), &personal)
            .await?;
        let organizations = self
            .auth
            .organization_choices(
                &Caller::new(subject.clone(), context.caller.authorization),
                token.expose(),
            )
            .await
            .map_err(|_| BotError::Unavailable)?;
        let mut choices = vec![SetupOwnerChoice {
            owner: personal,
            name: "Personal".into(),
        }];
        for organization in organizations
            .into_iter()
            .filter(|o| o.role == OrganizationRole::Admin)
        {
            choices.push(SetupOwnerChoice {
                name: organization
                    .display_name
                    .unwrap_or_else(|| organization.id.to_string()),
                owner: ResourceOwner::Organization {
                    subject: organization.id,
                },
            });
        }
        Ok(choices)
    }
    pub(super) async fn service_choices(
        &self,
        value: &ExternalActor,
        grant: &str,
        owner: &ResourceOwner,
    ) -> Result<Vec<SetupServiceChoice>, BotError> {
        let catalog = self.auth.services(grant, &actor(value), owner).await?;
        Ok(catalog
            .into_iter()
            .map(|service| SetupServiceChoice {
                required: self.required_services.contains(&service.slug),
                id: service.id,
                slug: service.slug,
                name: service.name,
                connected: service.connected,
                selected: false,
            })
            .collect())
    }
    pub(super) async fn channel_bots(
        &self,
        value: &ExternalActor,
        grant: &str,
        owner: &ResourceOwner,
    ) -> Result<Vec<SetupExternalBot>, BotError> {
        let (_, token) = self
            .auth
            .context_for_grant(grant, &actor(value), owner)
            .await?;
        self.connections
            .list(token.expose(), owner)
            .await
            .map(|items| {
                items
                    .into_iter()
                    .map(|item| SetupExternalBot {
                        id: item.connection_ref.clone(),
                        name: item.label.unwrap_or_else(|| "Channel".into()),
                        username: String::new(),
                        connection_ref: Some(item.connection_ref),
                    })
                    .collect()
            })
    }
}
