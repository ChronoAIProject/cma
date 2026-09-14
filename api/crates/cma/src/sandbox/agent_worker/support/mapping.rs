use layer2_core::profiles::SkillRef;
use layer2_core::sandbox::ResolvedAgentSkill;
use layer3_adapter::AgentSkill;

use super::WorkerFailure;
use crate::sandbox::agent_skills::validate_skill_selection;

pub(in crate::sandbox::agent_worker) fn validate_skills(
    references: &[SkillRef],
    resolved: &[ResolvedAgentSkill],
) -> Result<(), WorkerFailure> {
    validate_skill_selection(references, resolved).map_err(super::skill_failure)
}

pub(in crate::sandbox::agent_worker) fn validate_runtime_skills(
    material: &[ResolvedAgentSkill],
    skills: &[AgentSkill],
) -> Result<(), WorkerFailure> {
    if material.len() != skills.len()
        || !material.iter().zip(skills).all(|(selected, skill)| {
            selected.id.as_str() == skill.id
                && Some(selected.version.as_str()) == skill.version.as_deref()
                && selected.archive_sha256 == skill.archive_sha256
                && skill.source_url.len() <= 2048
                && reqwest::Url::parse(&skill.source_url).is_ok_and(|url| {
                    url.scheme() == "https"
                        && url.host_str().is_some()
                        && url.username().is_empty()
                        && url.password().is_none()
                        && url.fragment().is_none()
                })
        })
    {
        return Err(WorkerFailure::terminal(
            "agent_skill_invalid",
            "runtime skill locators changed immutable material",
        ));
    }
    Ok(())
}
