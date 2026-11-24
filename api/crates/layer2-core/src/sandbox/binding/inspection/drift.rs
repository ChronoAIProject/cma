use std::collections::BTreeSet;

use crate::profiles::SkillRef;
use crate::sandbox::{
    agent_live_skills, Agent, AgentInstructionPolicy, AgentSkillInstallStatus, AgentStatus,
};

use super::super::ProviderBinding;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftState {
    Match,
    Drift,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BindingProfileDrift {
    pub system_prompt: DriftState,
    pub skills: DriftState,
}

impl BindingProfileDrift {
    pub(in crate::sandbox::binding) fn compare(
        binding: &ProviderBinding,
        agent: Option<&Agent>,
        instruction_policy: &dyn AgentInstructionPolicy,
    ) -> Self {
        let Some(agent) =
            agent.filter(|agent| agent.ready_at.is_some() && agent.status != AgentStatus::Deleted)
        else {
            return Self {
                system_prompt: DriftState::Unknown,
                skills: DriftState::Unknown,
            };
        };
        Self {
            system_prompt: match &agent.conversation.settings {
                None => DriftState::Unknown,
                Some(_) => {
                    let baseline = &binding.profile.agent.system_prompt;
                    let matches = if binding.existing_agent.is_some() {
                        instruction_policy.matches_retained_prompt(agent, baseline)
                    } else {
                        agent.system_prompt() == instruction_policy.compose(agent, baseline)
                    };
                    if matches {
                        DriftState::Match
                    } else {
                        DriftState::Drift
                    }
                }
            },
            skills: compare_skills(agent, &binding.profile.agent.skills),
        }
    }
}

fn compare_skills(agent: &Agent, expected: &[SkillRef]) -> DriftState {
    let Some(initial) = agent.work.resolved_skills.as_ref() else {
        return DriftState::Unknown;
    };
    let observed_ids: BTreeSet<_> = initial
        .iter()
        .map(|skill| &skill.id)
        .chain(
            agent
                .work
                .live_skills
                .iter()
                .filter(|skill| skill.status == AgentSkillInstallStatus::Installed)
                .map(|skill| &skill.reference.id),
        )
        .collect();
    let expected_ids: BTreeSet<_> = expected.iter().map(|skill| &skill.id).collect();
    if observed_ids != expected_ids {
        return DriftState::Drift;
    }
    let mut state = DriftState::Match;
    for reference in expected {
        let Some(observed) = agent_live_skills::effective(&agent.work, &reference.id) else {
            return DriftState::Unknown;
        };
        if reference.source != observed.source {
            return DriftState::Drift;
        }
        match reference.version.as_deref() {
            Some(version) if version != observed.version => return DriftState::Drift,
            None => state = DriftState::Unknown,
            Some(_) => {}
        }
    }
    state
}
