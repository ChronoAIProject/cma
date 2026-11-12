use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Subject {
    AddWorktree {
        workspace: String,
        name: String,
        base: String,
    },
    RemoveWorktree {
        workspace: String,
        name: String,
    },
    InstallAgentSkill {
        request: layer3_adapter::AgentSkillInstallRequest,
    },
    CreateAgentHome {
        agent_id: String,
    },
    OpenAgent {
        agent_id: String,
        cwd: String,
        home: String,
    },
    CloseAgent {
        agent_id: String,
    },
    RemoveAgentHome {
        agent_id: String,
    },
}

#[derive(Debug, Clone, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Operation {
    pub operation_id: String,
    pub request_sha256: String,
    pub payload_sha256: String,
    pub subject: Subject,
}

impl PartialEq for Operation {
    fn eq(&self, other: &Self) -> bool {
        self.operation_id == other.operation_id
            && self.request_sha256 == other.request_sha256
            && self.payload_sha256 == other.payload_sha256
            && match (&self.subject, &other.subject) {
                (
                    Subject::InstallAgentSkill { request: a },
                    Subject::InstallAgentSkill { request: b },
                ) => {
                    a.has_valid_sha256()
                        && b.has_valid_sha256()
                        && a.request_sha256 == b.request_sha256
                }
                (a, b) => a == b,
            }
    }
}

impl Operation {
    pub(crate) fn install_agent_skill(request: &layer3_adapter::AgentSkillInstallRequest) -> Self {
        let mut request = request.clone();
        // The supervisor routes archives through its local proxy. Persist only
        // the path so a control-plane origin change cannot break durable replay.
        let mut skills = vec![&mut request.skill];
        if let Some(upgrade) = &mut request.upgrade {
            skills.push(&mut upgrade.previous);
        }
        for skill in skills {
            if let Ok(url) = url::Url::parse(&skill.source_url) {
                skill.source_url = url.path().to_owned();
                if let Some(query) = url.query() {
                    skill.source_url.push('?');
                    skill.source_url.push_str(query);
                }
            }
        }
        Self {
            operation_id: request.operation_id.clone(),
            request_sha256: request.request_sha256.clone(),
            payload_sha256: request.request_sha256.clone(),
            subject: Subject::InstallAgentSkill {
                request: request.clone(),
            },
        }
    }

    pub(crate) fn add_worktree(request: &layer3_adapter::WorktreeRequest) -> Self {
        Self {
            operation_id: request.operation_id.clone(),
            request_sha256: request.request_sha256.clone(),
            payload_sha256: request.request_sha256.clone(),
            subject: Subject::AddWorktree {
                workspace: request.workspace.clone(),
                name: request.name.clone(),
                base: request.base.clone(),
            },
        }
    }

    pub(crate) fn remove_worktree(request: &layer3_adapter::WorktreeRef) -> Self {
        Self {
            operation_id: request.operation_id.clone(),
            request_sha256: request.request_sha256.clone(),
            payload_sha256: request.request_sha256.clone(),
            subject: Subject::RemoveWorktree {
                workspace: request.workspace.clone(),
                name: request.name.clone(),
            },
        }
    }

    pub(crate) fn create_agent_home(plan: &layer3_adapter::AgentHomePlan) -> Self {
        Self {
            operation_id: plan.operation_id.clone(),
            request_sha256: plan.request_sha256.clone(),
            payload_sha256: plan.payload_sha256(),
            subject: Subject::CreateAgentHome {
                agent_id: plan.agent_id.as_str().into(),
            },
        }
    }

    pub(crate) fn open_agent(launch: &layer3_adapter::AgentLaunch) -> Self {
        Self {
            operation_id: launch.operation_id.clone(),
            request_sha256: launch.request_sha256.clone(),
            payload_sha256: launch.payload_sha256(),
            subject: Subject::OpenAgent {
                agent_id: launch.agent_id.as_str().into(),
                cwd: launch.cwd.clone(),
                home: launch.home.clone(),
            },
        }
    }

    pub(crate) fn close_agent(request: &layer3_adapter::AgentCloseRequest) -> Self {
        Self {
            operation_id: request.operation_id.clone(),
            request_sha256: request.request_sha256.clone(),
            payload_sha256: request.request_sha256.clone(),
            subject: Subject::CloseAgent {
                agent_id: request.agent_id.as_str().into(),
            },
        }
    }

    pub(crate) fn remove_agent_home(request: &layer3_adapter::AgentHomeRemoveRequest) -> Self {
        Self {
            operation_id: request.operation_id.clone(),
            request_sha256: request.request_sha256.clone(),
            payload_sha256: request.request_sha256.clone(),
            subject: Subject::RemoveAgentHome {
                agent_id: request.agent_id.as_str().into(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActiveAgent {
    pub agent_id: String,
    pub cwd: String,
    pub home: String,
}
