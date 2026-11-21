//! Reconstruct receipts from immutable material, never from a fresh catalog lookup.
use layer2_core::sandbox::{Agent, AgentSkillInstallStatus, ResolvedAgentSkill};
use layer3_adapter::{AgentSkill, AgentSkillInstallReceipt, AgentSkillInstallRequest, AgentSlot};
use std::collections::BTreeMap;

pub(super) fn request(
    agent: &Agent,
    current: AgentSkill,
    previous: Option<AgentSkill>,
) -> AgentSkillInstallRequest {
    let slot = AgentSlot::new(agent.id.as_str());
    let mut receipts = BTreeMap::new();
    for (index, installation) in agent.work.live_skills.iter().enumerate() {
        let last = index + 1 == agent.work.live_skills.len();
        let Some(material) = &installation.material else {
            continue;
        };
        let target = if last {
            current.clone()
        } else {
            identity(material)
        };
        let request = match &installation.previous {
            Some(old) => AgentSkillInstallRequest::upgrade(
                slot.clone(),
                target,
                if last {
                    previous.clone().expect("validated previous locator")
                } else {
                    identity(old)
                },
                receipts.get(&material.id).cloned(),
                (index + 1) as u64,
            ),
            None => AgentSkillInstallRequest::new(slot.clone(), target),
        };
        if last {
            return request;
        }
        if installation.status == AgentSkillInstallStatus::Installed {
            receipts.insert(
                material.id.clone(),
                AgentSkillInstallReceipt::from_request(&request),
            );
        }
    }
    unreachable!("current material is durably pinned before constructing a request")
}
fn identity(material: &ResolvedAgentSkill) -> AgentSkill {
    AgentSkill {
        id: material.id.as_str().into(),
        name: material.name.clone(),
        version: Some(material.version.clone()),
        archive_sha256: material.archive_sha256.clone(),
        source_url: String::new(),
    }
}
