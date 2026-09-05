use layer2_core::profiles::SkillRef;
use layer3_adapter::AgentSkill;

use super::WorkerFailure;

pub(in crate::sandbox::agent_worker) fn validate_skills(
    references: &[SkillRef],
    resolved: &[AgentSkill],
) -> Result<(), WorkerFailure> {
    let matches = references.len() == resolved.len()
        && references.iter().zip(resolved).all(|(reference, skill)| {
            skill.id == reference.id.as_str()
                && skill.version == reference.version
                && valid_sha256(&skill.archive_sha256)
                && skill.source_url.len() <= 2_048
                && reqwest::Url::parse(&skill.source_url)
                    .is_ok_and(|url| url.scheme() == "https" && url.host_str().is_some())
        });
    if matches {
        Ok(())
    } else {
        Err(WorkerFailure::terminal(
            "agent_skill_invalid",
            "skill resolution did not match the pinned agent profile",
        ))
    }
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
