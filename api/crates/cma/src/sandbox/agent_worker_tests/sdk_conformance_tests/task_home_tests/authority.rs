use super::*;
use layer2_core::sandbox::task_experience::TaskExperiencePolicy;

// Mirror production injection while limiting rollout authority to this fixture's owner.
pub(super) fn task_experience_policy() -> Arc<TaskExperiencePolicy> {
    Arc::new(
        serde_json::from_value(json!({
            "mode": "accounts", "accounts": ["owner"],
            "capabilities": ["task_home_admission", "desktop_enable"]
        }))
        .unwrap(),
    )
}

pub(super) struct PersonalAuthority;

#[async_trait::async_trait]
impl OrganizationAuthority for PersonalAuthority {
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
