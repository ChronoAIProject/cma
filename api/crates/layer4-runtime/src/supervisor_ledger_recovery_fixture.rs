use super::*;
use layer3_adapter::{AgentSkill, AgentSkillInstallRequest, SkillArchive};
use sha2::{Digest, Sha256};
use std::io::Write;

#[derive(Clone, Copy, Debug)]
pub(super) enum Case {
    Close,
    Remove,
    Add,
    AddFailed,
    AddIndeterminate,
    Skill,
    SkillFailed,
}

impl Case {
    pub(super) const ALL: [Self; 7] = [
        Self::Close,
        Self::Remove,
        Self::Add,
        Self::AddFailed,
        Self::AddIndeterminate,
        Self::Skill,
        Self::SkillFailed,
    ];
    pub(super) fn status(self) -> Status {
        match self {
            Self::AddFailed | Self::SkillFailed => Status::Failed,
            Self::AddIndeterminate => Status::Indeterminate,
            _ => Status::Completed,
        }
    }
}

pub(super) async fn prepare(
    case: Case,
    supervisor: &Arc<Supervisor>,
    ledger: &Arc<Ledger>,
) -> Operation {
    match case {
        Case::Close => Operation::close_agent(&AgentCloseRequest::new(
            "agent:probe:close:v1",
            AgentSlot::new("probe"),
        )),
        Case::Remove | Case::Add | Case::AddFailed | Case::AddIndeterminate => {
            let workspace = prepare_workspace(supervisor).await;
            if matches!(case, Case::Remove) {
                add_worktree(supervisor, workspace.resolved_commit).await;
                return Operation::remove_worktree(&WorktreeRef::new(
                    "worktree:project:agent-1:remove:v1",
                    "project",
                    "agent-1",
                ));
            }
            if matches!(case, Case::AddIndeterminate) {
                std::fs::create_dir_all(crate::worktree::expected_path(
                    &supervisor.config.home,
                    "project",
                    "agent-1",
                ))
                .unwrap();
            }
            Operation::add_worktree(&WorktreeRequest::new(
                "worktree:project:agent-1:add:v1",
                "project",
                "agent-1",
                if matches!(case, Case::AddFailed) {
                    "f".repeat(40)
                } else {
                    workspace.resolved_commit
                },
            ))
        }
        Case::Skill | Case::SkillFailed => {
            let plan = home_plan("agent-1", b"preserved home");
            let home = Operation::create_agent_home(&plan);
            ledger.admit(1, vec![home]).await.unwrap();
            assert!(matches!(
                supervisor.dispatch(materialize_home_request(plan)).await,
                ResponseFrame::MaterializeAgentHome(_)
            ));
            let root = supervisor.config.agent_home("agent-1").join("skills");
            std::fs::create_dir_all(&root).unwrap();
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
            zip.start_file("SKILL.md", zip::write::SimpleFileOptions::default())
                .unwrap();
            zip.write_all(b"# Recovery test skill\n").unwrap();
            let bytes = zip.finish().unwrap().into_inner();
            let archive = SkillArchive {
                sha256: format!("{:x}", Sha256::digest(&bytes)),
                bytes,
            };
            crate::skills::cache_archive(&supervisor.config, &archive).unwrap();
            let request = AgentSkillInstallRequest::new(
                AgentSlot::new("agent-1"),
                AgentSkill {
                    id: "recovery".into(),
                    name: None,
                    version: Some("1.0".into()),
                    archive_sha256: archive.sha256,
                    source_url: "https://unreachable.invalid/recovery.zip".into(),
                },
            );
            if matches!(case, Case::SkillFailed) {
                std::fs::write(root.join("recovery"), b"preserve conflict").unwrap();
            }
            Operation::install_agent_skill(&request)
        }
    }
}

pub(super) fn effect_preserved(case: Case, config: &RuntimeConfig) -> bool {
    let worktree = crate::worktree::expected_path(&config.home, "project", "agent-1");
    match case {
        Case::Close => true,
        Case::Remove | Case::AddFailed => !worktree.exists(),
        Case::Add => worktree.join(".git").is_file(),
        Case::AddIndeterminate => worktree.is_dir() && !worktree.join(".git").exists(),
        Case::Skill => {
            std::fs::read(
                config
                    .agent_home("agent-1")
                    .join("skills/recovery/SKILL.md"),
            )
            .ok()
            .as_deref()
                == Some(b"# Recovery test skill\n")
        }
        Case::SkillFailed => {
            std::fs::read(config.agent_home("agent-1").join("skills/recovery"))
                .ok()
                .as_deref()
                == Some(b"preserve conflict")
        }
    }
}
