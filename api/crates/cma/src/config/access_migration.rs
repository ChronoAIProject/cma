//! Explicit, disabled-by-default batches for retained Agent tool transport.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AgentAccessMigration {
    pub revision: u64,
    pub origin: String,
    pub agents: Vec<String>,
}

impl AgentAccessMigration {
    pub(crate) fn includes(&self, agent: &str) -> bool {
        self.agents.iter().any(|id| id == "*" || id == agent)
    }
}

pub(crate) fn parse(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<Option<AgentAccessMigration>, String> {
    let key = "CHRONO_AGENT_ACCESS_TRANSPORT_MIGRATION";
    let raw = lookup("CHRONO_AGENT_ACCESS_TRANSPORT_MIGRATION")?.unwrap_or_default();
    if raw.is_empty() {
        return Ok(None);
    }
    let mut value: AgentAccessMigration = serde_json::from_str(&raw)
        .map_err(|_| format!("{key} requires revision, origin and agents"))?;
    value.origin = super::parse_canonical_https_origin(key, &value.origin)?;
    let all = value.agents.as_slice() == ["*"];
    if value.revision < 2
        || value.revision > i64::MAX as u64
        || value.agents.is_empty()
        || value.agents.len() > 64
        || (!all
            && value.agents.iter().any(|id| {
                id.strip_prefix("agt_").is_none_or(|s| {
                    s.len() != 32
                        || !s
                            .bytes()
                            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                })
            }))
    {
        return Err(format!(
            "{key} needs revision >= 2 and 1-64 exact Agent IDs, or only '*'"
        ));
    }
    value.agents.sort();
    value.agents.dedup();
    Ok(Some(value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_migration_is_explicit_bounded_and_requires_a_later_revision() {
        assert!(parse(&mut |_| Ok(None)).unwrap().is_none());
        let id = "agt_0123456789abcdef0123456789abcdef";
        let valid = serde_json::json!({"revision":2,"origin":"https://cma.example","agents":[id]});
        let parsed = parse(&mut |_| Ok(Some(valid.to_string())))
            .unwrap()
            .unwrap();
        assert!(parsed.includes(id));
        assert!(!parsed.includes("agt_other"));
        for invalid in [
            serde_json::json!({"revision":1,"origin":"https://cma.example","agents":[id]}),
            serde_json::json!({"revision":2,"origin":"http://cma.example","agents":[id]}),
            serde_json::json!({"revision":2,"origin":"https://cma.example/path","agents":[id]}),
            serde_json::json!({"revision":2,"origin":"https://cma.example","agents":[]}),
            serde_json::json!({"revision":2,"origin":"https://cma.example","agents":["*",id]}),
            serde_json::json!({"revision":2,"origin":"https://cma.example","agents":["agt_wrong"]}),
        ] {
            assert!(parse(&mut |_| Ok(Some(invalid.to_string()))).is_err());
        }
        let all = serde_json::json!({"revision":3,"origin":"https://cma.example","agents":["*"]});
        assert!(parse(&mut |_| Ok(Some(all.to_string())))
            .unwrap()
            .unwrap()
            .includes(id));
    }
}
