use super::*;
use futures_util::TryStreamExt;
use layer2_core::{
    sandbox::Sandbox,
    sandbox_metrics::{ReportQuery, Scope},
    sandbox_usage::SandboxUsage,
};
use std::collections::BTreeMap;

pub(crate) struct Inventory {
    pub state: SandboxUsage,
    pub sandbox: Option<Sandbox>,
    pub agent_count: u64,
}
impl ReportingStore {
    pub(crate) async fn inventory(
        &self,
        session: &mut mongodb::ClientSession,
        scope: &Scope,
        query: &ReportQuery,
        after: Option<&str>,
    ) -> Result<Vec<Inventory>> {
        let _timer = self.time_read("inventory")?;
        let mut filter = Document::new();
        if let Some(owner) = if scope.admin {
            query.filters.owner.as_deref()
        } else {
            Some(scope.principal.as_str())
        } {
            filter.insert("owner", owner);
        }
        if let Some(id) = &query.filters.sandbox_id {
            if after.is_some_and(|after| after >= id.as_str()) {
                return Ok(Vec::new());
            }
            filter.insert("_id", id);
        } else if let Some(after) = after {
            filter.insert("_id", doc! {"$gt":after});
        }
        let mut cursor = self
            .collection(super::super::sandbox_usage::STATES)
            .find(filter)
            .sort(doc! {"_id":1})
            .limit(200)
            .batch_size(200)
            .max_time(DB_DEADLINE)
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        let mut states = Vec::new();
        while let Some(doc) = cursor
            .next(&mut *session)
            .await
            .transpose()
            .map_err(unavailable)?
        {
            states.push(super::super::sandbox_usage::read_state(&doc).map_err(unavailable)?);
        }
        let ids: Vec<_> = states.iter().map(|s| s.sandbox_id.as_str()).collect();
        let mut sandboxes = BTreeMap::new();
        let mut cursor = self
            .collection("sandboxes")
            .find(doc! {"_id":{"$in":&ids}})
            .max_time(DB_DEADLINE)
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        while let Some(doc) = cursor
            .next(&mut *session)
            .await
            .transpose()
            .map_err(unavailable)?
        {
            let sandbox = super::super::sandboxes::read_sandbox(&doc).map_err(unavailable)?;
            if states
                .iter()
                .any(|s| s.sandbox_id == sandbox.id && s.owner != sandbox.owner)
            {
                return Err(MetricsError::Unavailable);
            }
            sandboxes.insert(sandbox.id.to_string(), sandbox);
        }
        let mut counts = BTreeMap::new();
        let mut cursor = self
            .collection("agents")
            .aggregate([
                doc! {"$match":{"sandbox_id":{"$in":&ids},"status":{"$ne":"retired"}}},
                doc! {"$group":{"_id":"$sandbox_id","count":{"$sum":1_i64}}},
            ])
            .max_time(DB_DEADLINE)
            .session(&mut *session)
            .await
            .map_err(unavailable)?;
        while let Some(doc) = cursor
            .next(&mut *session)
            .await
            .transpose()
            .map_err(unavailable)?
        {
            counts.insert(
                doc.get_str("_id").map_err(unavailable)?.to_owned(),
                doc.get_i64("count").map_err(unavailable)? as u64,
            );
        }
        Ok(states
            .into_iter()
            .map(|state| Inventory {
                sandbox: sandboxes.remove(state.sandbox_id.as_str()),
                agent_count: counts.remove(state.sandbox_id.as_str()).unwrap_or(0),
                state,
            })
            .collect())
    }
    pub(crate) async fn observation_page(&self, after: Option<&str>) -> Result<Vec<Sandbox>> {
        let mut filter = doc! {"record.lifecycle.home.receipt":{"$ne":null},"record.lifecycle.home.released":{"$ne":true}};
        if let Some(after) = after {
            filter.insert("_id", doc! {"$gt":after});
        }
        let mut cursor = self
            .collection("sandboxes")
            .find(filter)
            .sort(doc! {"_id":1})
            .limit(200)
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?;
        let mut rows = Vec::new();
        while let Some(doc) = cursor.try_next().await.map_err(unavailable)? {
            rows.push(super::super::sandboxes::read_sandbox(&doc).map_err(unavailable)?);
        }
        Ok(rows)
    }
    pub(crate) async fn authority_unchanged(&self, sandbox: &Sandbox) -> Result<bool> {
        Ok(self.collection("sandboxes").find_one(doc!{"_id":sandbox.id.as_str(),"owner":sandbox.owner.as_str(),"revision":sandbox.revision as i64}).projection(doc!{"_id":1}).max_time(DB_DEADLINE).await.map_err(unavailable)?.is_some())
    }
}
impl Inventory {
    pub(crate) fn matches(&self, query: &ReportQuery) -> bool {
        let filters = &query.filters;
        (filters.statuses.is_empty()
            || filters.statuses.iter().any(|s| {
                serde_json::to_value(s)
                    .ok()
                    .and_then(|s| s.as_str().map(str::to_owned))
                    .as_deref()
                    == Some(&self.state.status)
            }))
            && filters.runtime_version.as_ref().is_none_or(|v| {
                self.sandbox
                    .as_ref()
                    .and_then(|s| {
                        s.runtime
                            .as_ref()
                            .or(s.lifecycle.suspended_runtime.as_deref())
                    })
                    .is_some_and(|r| &r.runtime_version == v)
            })
            && (filters.search.is_empty()
                || self
                    .state
                    .name
                    .trim_start()
                    .to_lowercase()
                    .starts_with(&filters.search)
                || matches_sandbox_id_search(self.state.sandbox_id.as_str(), &filters.search))
    }
}

fn matches_sandbox_id_search(id: &str, search: &str) -> bool {
    if id.starts_with(search) {
        return true;
    }
    let Some(opaque_id) = id.strip_prefix("sbx_").filter(|opaque_id| {
        opaque_id.len() == 32 && opaque_id.bytes().all(|byte| byte.is_ascii_hexdigit())
    }) else {
        return false;
    };
    if opaque_id.starts_with(search) {
        return true;
    }
    // Usage renders long IDs with a literal ellipsis. Require both displayed
    // ends so an abbreviated value selects exactly the visible identity.
    search.split_once('…').is_some_and(|(prefix, suffix)| {
        prefix.len() == 14 && suffix.len() == 8 && id.starts_with(prefix) && id.ends_with(suffix)
    })
}
