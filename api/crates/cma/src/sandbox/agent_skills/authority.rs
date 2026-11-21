use super::*;
use crate::auth::persistence::SecretValue;
use crate::sandbox::credential::CredentialError;

impl OrnnAgentSkillResolver {
    pub(super) async fn scoped_bearer(
        &self,
        sandbox: &Sandbox,
    ) -> Result<Option<SecretValue>, AgentSkillResolutionError> {
        let Some(credentials) = &self.credentials else {
            return Ok(None);
        };
        if sandbox.validate().is_err()
            || !sandbox.credential.delivered
            || !sandbox
                .credential
                .effective_reaches()
                .iter()
                .any(|service| service.as_str() == "ornn-api")
        {
            return Err(invalid(
                0,
                "sandbox has no valid admitted Ornn credential scope",
            ));
        }
        let current = sandbox
            .credential
            .current
            .as_ref()
            .ok_or_else(|| invalid(0, "sandbox credential authority is missing"))?;
        let material = credentials
            .material(&sandbox.id, current.generation)
            .await
            .map_err(|error| match error {
                CredentialError::Unavailable(_) => {
                    unavailable("sandbox credential custody is unavailable")
                }
                _ => invalid(0, "sandbox credential custody was refused"),
            })?
            .ok_or_else(|| unavailable("sandbox credential material is unavailable"))?;
        if material.sandbox_id != sandbox.id
            || material.owner != sandbox.owner
            || &material.authority != current
            || material.value.expose().trim().is_empty()
        {
            return Err(invalid(
                0,
                "sandbox credential material changed admitted authority",
            ));
        }
        Ok(Some(material.value))
    }
}
