use sha2::{Digest, Sha256};

use super::{Member, Membership, OwnershipError, PROTOCOL_VERSION};
use crate::AgentId;

/// Stable rendezvous order, independent of discovery order and replica count.
/// An existing owner remains eligible at its limit: it already occupies a slot.
/// Only the current owner may release for rebalance; this function grants no lease.
pub fn candidates<'a>(
    agent: &AgentId,
    membership: &'a Membership,
    current_owner: Option<&str>,
) -> Result<Vec<&'a Member>, OwnershipError> {
    membership.require_compatible()?;
    let mut ranked: Vec<_> = membership
        .members
        .iter()
        .filter(|member| {
            member.is_live(membership.now_ms)
                && !member.draining
                && member.protocol == PROTOCOL_VERSION
                && (member.owned_agents < member.max_owned_agents
                    || current_owner == Some(member.instance_id.as_str()))
        })
        .map(|member| (score(agent, &member.instance_id), member))
        .collect();
    ranked.sort_unstable_by(|(left_score, left), (right_score, right)| {
        right_score
            .cmp(left_score)
            .then_with(|| left.instance_id.cmp(&right.instance_id))
    });
    Ok(ranked.into_iter().map(|(_, member)| member).collect())
}

fn score(agent: &AgentId, instance_id: &str) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"cma-agent-manager-placement-v1\0");
    digest.update((agent.as_str().len() as u64).to_be_bytes());
    digest.update(agent.as_str().as_bytes());
    digest.update(instance_id.as_bytes());
    digest.finalize().into()
}
