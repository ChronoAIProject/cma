use super::*;

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
