//! Preserve reservation and process projections alongside the journal sources.
use super::{authority, field, invalid, Kind, ResourceKey, Result, RuntimeConfig, Sha256};
use sha2::Digest;

pub(super) const CLEANUP: &str = "SELECT scope,resource,available,NULL,NULL,NULL,
    length(scope)+length(CAST(resource AS BLOB))+1 FROM cleanup_budget
    WHERE (scope,resource)>(?,?) ORDER BY scope,resource LIMIT 65";
pub(super) const SCOPES: &str = "SELECT scope,'',pending,processes,NULL,NULL,length(scope)+16
    FROM scope_budget WHERE scope>? ORDER BY scope LIMIT 65";
pub(super) const PROCESSES: &str = "SELECT scope,agent,cwd,home,NULL,NULL,
    length(scope)+length(CAST(agent AS BLOB))+length(CAST(cwd AS BLOB))+length(CAST(home AS BLOB))
    FROM logical_processes WHERE (scope,agent)>(?,?) ORDER BY scope,agent LIMIT 65";

#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Auxiliary {
    Cleanup {
        scope: Vec<u8>,
        resource: String,
        available: bool,
    },
    Scope {
        scope: Vec<u8>,
        pending: i64,
        processes: i64,
    },
    Process {
        scope: Vec<u8>,
        agent: String,
        cwd: String,
        home: String,
    },
}

impl Auxiliary {
    pub(super) fn read(row: &rusqlite::Row<'_>, kind: Kind) -> Result<Self> {
        let scope = row.get(0).map_err(|_| invalid())?;
        Ok(match kind {
            Kind::Cleanup => Self::Cleanup {
                scope,
                resource: row.get(1).map_err(|_| invalid())?,
                available: match row.get::<_, i64>(2).map_err(|_| invalid())? {
                    0 => false,
                    1 => true,
                    _ => return Err(invalid()),
                },
            },
            Kind::Scopes => Self::Scope {
                scope,
                pending: row.get(2).map_err(|_| invalid())?,
                processes: row.get(3).map_err(|_| invalid())?,
            },
            Kind::Processes => Self::Process {
                scope,
                agent: row.get(1).map_err(|_| invalid())?,
                cwd: row.get(2).map_err(|_| invalid())?,
                home: row.get(3).map_err(|_| invalid())?,
            },
            _ => return Err(invalid()),
        })
    }

    pub(super) fn key(&self) -> (&[u8], &str) {
        match self {
            Self::Cleanup {
                scope, resource, ..
            } => (scope, resource),
            Self::Scope { scope, .. } => (scope, ""),
            Self::Process { scope, agent, .. } => (scope, agent),
        }
    }

    pub(super) fn bytes(&self) -> usize {
        let (scope, key) = self.key();
        scope.len()
            + key.len()
            + match self {
                Self::Cleanup { .. } => 1,
                Self::Scope { .. } => 16,
                Self::Process { cwd, home, .. } => cwd.len() + home.len(),
            }
    }

    pub(super) fn hash_into(&self, digest: &mut Sha256) {
        let (scope, key) = self.key();
        digest.update([match self {
            Self::Cleanup { .. } => 5,
            Self::Scope { .. } => 6,
            Self::Process { .. } => 7,
        }]);
        field(digest, Some(scope));
        field(digest, Some(key.as_bytes()));
        match self {
            Self::Cleanup { available, .. } => digest.update([u8::from(*available)]),
            Self::Scope {
                pending, processes, ..
            } => {
                digest.update(pending.to_be_bytes());
                digest.update(processes.to_be_bytes());
            }
            Self::Process { cwd, home, .. } => {
                field(digest, Some(cwd.as_bytes()));
                field(digest, Some(home.as_bytes()));
            }
        }
    }

    pub(super) fn validate(&self, config: &RuntimeConfig, generation: u64) -> Result<()> {
        let component = |name: &str| {
            crate::paths::safe_component(name, "ledger component").map_err(|_| invalid())
        };
        let process_scope = |is_process| {
            if (generation != 0) != (is_process && config.home_identity.is_some()) {
                Err(invalid())
            } else {
                Ok(())
            }
        };
        match self {
            Self::Cleanup { resource, .. } => {
                if resource.len() > 65536 {
                    return Err(invalid());
                }
                let key: ResourceKey = serde_json::from_str(resource).map_err(|_| invalid())?;
                if authority::key(&key)? != *resource {
                    return Err(invalid());
                }
                process_scope(matches!(key, ResourceKey::AgentProcess { .. }))?;
                match key {
                    ResourceKey::Worktree { workspace, name } => {
                        component(&workspace)?;
                        component(&name)?;
                    }
                    ResourceKey::AgentHome { agent_id }
                    | ResourceKey::AgentProcess { agent_id, .. } => component(&agent_id)?,
                    // Skills reserve cleanup against their parent home.
                    ResourceKey::AgentSkill { .. } => return Err(invalid()),
                }
            }
            Self::Scope {
                pending, processes, ..
            } => {
                if *pending < 0
                    || *processes < 0
                    || (generation > 0 && config.home_identity.is_none())
                {
                    return Err(invalid());
                }
            }
            Self::Process {
                agent, cwd, home, ..
            } => {
                component(agent)?;
                process_scope(true)?;
                if cwd.len() > 65536
                    || home.len() > 65536
                    || std::path::Path::new(home) != config.agent_home(agent)
                    || crate::worktree::parse_path(&config.home, std::path::Path::new(cwd))
                        .is_none()
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }
}
