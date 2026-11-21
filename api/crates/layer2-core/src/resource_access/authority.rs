use serde::Serialize;

use crate::{Caller, SubjectId};

use super::{CallerContext, MembershipError, OrganizationRole};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OrganizationChoice {
    pub id: SubjectId,
    pub role: OrganizationRole,
    pub display_name: Option<String>,
}

/// Request-bound identity provider port. Credentials are supplied only by the
/// authenticated boundary/custody adapter and never stored in these results.
#[async_trait::async_trait]
pub trait OrganizationAuthority: Send + Sync {
    async fn organization_context(
        &self,
        caller: &Caller,
        credential: &str,
        organization: &SubjectId,
    ) -> Result<CallerContext, MembershipError>;

    /// Presentation inventory is not reusable membership evidence.
    async fn organization_choices(
        &self,
        caller: &Caller,
        credential: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError>;
}
