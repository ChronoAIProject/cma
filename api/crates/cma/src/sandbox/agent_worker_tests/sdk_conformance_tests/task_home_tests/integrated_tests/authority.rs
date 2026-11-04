use super::*;
use layer2_core::sandbox::agent_io::*;

pub(super) struct Authority;

pub(super) async fn authenticate(mut request: Request, next: Next) -> Result<Response, StatusCode> {
    if request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(TEST_BEARER)
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let owner = request
        .headers()
        .get("x-integrated-subject")
        .and_then(|value| value.to_str().ok())
        .unwrap_or("owner");
    if !matches!(owner, "owner" | "other") {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let owner = SubjectId::new(owner);
    let authorization = Authorization::new(
        if request
            .headers()
            .get("x-integrated-revoked")
            .and_then(|value| value.to_str().ok())
            == Some("true")
        {
            vec![]
        } else {
            vec![
                Capability::UseOwnSandboxes,
                Capability::CreateSandbox,
                Capability::ManageOwnProfiles,
            ]
        },
    );
    let mut context = v2_tests::context(&owner);
    context.caller = layer2_core::Caller::new(owner.clone(), authorization.clone());
    request
        .extensions_mut()
        .insert(layer1_public::AuthenticatedSubject(owner));
    request.extensions_mut().insert(context);
    request
        .extensions_mut()
        .insert(layer1_public::DelegatedCredential::new(
            "local-task-owner".into(),
        ));
    request.extensions_mut().insert(authorization);
    Ok(next.run(request).await)
}

#[async_trait::async_trait]
impl PersonalCommandAdmissionAuthority for Authority {
    async fn personal_command_authority(
        &self,
        caller: &layer2_core::Caller,
        credential: &str,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        if caller.subject.as_str() != "owner"
            || credential != "local-task-owner"
            || !caller.authorization.allows(Capability::UseOwnSandboxes)
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(AgentAuthorizationRevision {
            authority_id: "ticket19-owner".into(),
            policy_revision: 1,
            grant_revision: None,
        })
    }
}

#[async_trait::async_trait]
impl OrganizationAuthority for Authority {
    async fn organization_context(
        &self,
        _: &layer2_core::Caller,
        _: &str,
        _: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        Err(MembershipError::NotMember)
    }
    async fn organization_choices(
        &self,
        _: &layer2_core::Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        Ok(vec![])
    }
}

#[async_trait::async_trait]
impl AgentCommandAdmissionAuthority for Authority {
    async fn command_admission_authority(
        &self,
        caller: &CallerContext,
        credential: &str,
        owner: &AgentOwner,
    ) -> Result<AgentAuthorizationRevision, CommandAuthorizationError> {
        if caller.caller.subject.as_str() != "owner"
            || credential != "local-task-owner"
            || owner
                != &(AgentOwner::Person {
                    subject: SubjectId::new("owner"),
                })
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(AgentAuthorizationRevision {
            authority_id: "ticket19-owner".into(),
            policy_revision: 1,
            grant_revision: None,
        })
    }
}

#[async_trait::async_trait]
impl AgentCommandAuthorizer for Authority {
    async fn authorize_command(
        &self,
        command: &AgentCommandEnvelope,
    ) -> Result<layer2_core::Caller, CommandAuthorizationError> {
        if command.authorization.authority_id != "ticket19-owner"
            || command.owner
                != (AgentOwner::Person {
                    subject: SubjectId::new("owner"),
                })
        {
            return Err(CommandAuthorizationError::Revoked);
        }
        Ok(layer2_core::Caller::new(
            SubjectId::new("owner"),
            Authorization::new([Capability::UseOwnSandboxes]),
        ))
    }
}
