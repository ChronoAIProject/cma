use super::{
    rejected, retirement_inventory::owners, AgentSandboxPools, NativePoolBinding, NativePoolClaim,
    Result,
};
use kube::{api::DynamicObject, ResourceExt};

const POOL_LABEL: &str = "agents.x-k8s.io/warm-pool-sandbox";
const ASSIGNMENT: &str = "agents.x-k8s.io/sandbox-name";

impl AgentSandboxPools {
    pub(super) fn prove_drain_assignment(
        &self,
        binding: &NativePoolBinding,
        expected: &NativePoolClaim,
        claim: &DynamicObject,
        member: &DynamicObject,
        members: &[DynamicObject],
    ) -> Result<()> {
        let name = member.name_any();
        self.cleanup_member(member, expected)?;
        if expected.request.pool != *binding
            || member.metadata.deletion_timestamp.is_some()
            || member.labels().contains_key(POOL_LABEL)
            || member
                .data
                .pointer("/spec/podTemplate/metadata/labels/agents.x-k8s.io~1claim-uid")
                .and_then(|label| label.as_str())
                != Some(&expected.uid)
            || assignment_names(claim).any(|assigned| assigned != name)
            || members.iter().any(|other| {
                other.name_any() != name
                    && owners(other)
                        .iter()
                        .any(|owner| owner.uid == expected.uid || owner.name == expected.name)
            })
        {
            return Err(rejected("native drain Claim ownership is ambiguous"));
        }
        Ok(())
    }
}

fn assignment_names(claim: &DynamicObject) -> impl Iterator<Item = &str> {
    [
        claim
            .data
            .pointer("/status/sandbox/name")
            .and_then(|v| v.as_str()),
        claim.annotations().get(ASSIGNMENT).map(String::as_str),
        claim.labels().get(ASSIGNMENT).map(String::as_str),
    ]
    .into_iter()
    .flatten()
}
