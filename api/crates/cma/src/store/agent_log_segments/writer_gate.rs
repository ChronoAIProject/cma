//! Durable, independent rollout cohorts. Publication admission writes the same
//! gate document as closure, so a stale process cannot admit through an old read.
use layer2_core::sandbox::{store::SandboxStoreError, Agent};
use mongodb::{
    bson::{self, doc, Document},
    ClientSession, Database,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::store::agent_log_integrity::TransactionLogError;
use crate::store::sandbox_mongo::is_duplicate_key;

pub(in crate::store) const COLLECTION: &str = "agent_log_gates";
const MAX_COHORT: usize = 64;
const MAX_POLICY_JSON: usize = 48 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(in crate::store) enum GateName {
    #[serde(rename = "agent_log_segments_v1")]
    Segments,
    #[serde(rename = "agent_log_capacity_recovery_v1")]
    CapacityRecovery,
}

impl GateName {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Segments => "agent_log_segments_v1",
            Self::CapacityRecovery => "agent_log_capacity_recovery_v1",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct Policy {
    pub enabled: bool,
    pub agents: Vec<String>,
    pub owners: Vec<String>,
}

impl Policy {
    pub fn parse(json: &str) -> Result<Self, SandboxStoreError> {
        if json.len() > MAX_POLICY_JSON {
            return Err(invalid());
        }
        let policy: Self = serde_json::from_str(json).map_err(|_| invalid())?;
        policy.validate()?;
        Ok(policy)
    }

    fn validate(&self) -> Result<(), SandboxStoreError> {
        let count = self
            .agents
            .len()
            .checked_add(self.owners.len())
            .ok_or_else(invalid)?;
        if count > MAX_COHORT || (self.enabled && count == 0) {
            return Err(invalid());
        }
        for group in [&self.agents, &self.owners] {
            let mut seen = HashSet::new();
            for id in group {
                if id.is_empty()
                    || id.len() > 512
                    || id == "*"
                    || id.chars().any(|c| c.is_control() || c.is_whitespace())
                    || !seen.insert(id)
                {
                    return Err(invalid());
                }
            }
        }
        Ok(())
    }

    pub fn selects(&self, agent_id: &str, owner: &str) -> bool {
        self.enabled
            && (self.agents.iter().any(|id| id == agent_id)
                || self.owners.iter().any(|id| id == owner))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct GateRecord {
    #[serde(rename = "_id")]
    pub name: GateName,
    pub format_version: i64,
    pub revision: i64,
    pub admission_fence: i64,
    pub policy: Policy,
}

impl GateRecord {
    pub fn read(name: GateName, document: Document) -> Result<Self, SandboxStoreError> {
        let gate: Self = bson::deserialize_from_document(document).map_err(|_| invalid())?;
        if gate.name != name
            || gate.format_version != 1
            || gate.revision < 1
            || gate.admission_fence < 0
        {
            return Err(invalid());
        }
        gate.policy.validate()?;
        Ok(gate)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(in crate::store) struct GateProof {
    pub name: GateName,
    pub revision: i64,
}

pub(in crate::store) async fn read(
    database: &Database,
    name: GateName,
    session: &mut ClientSession,
) -> Result<Option<GateRecord>, TransactionLogError> {
    database
        .collection::<Document>(COLLECTION)
        .find_one(doc! {"_id":name.as_str()})
        .session(session)
        .await
        .map_err(TransactionLogError::Driver)?
        .map(|row| GateRecord::read(name, row))
        .transpose()
        .map_err(Into::into)
}

/// Absence is closed. A successfully selected gate must be write-fenced in the
/// transaction that inserts the exact immutable publication admission.
pub(in crate::store) async fn select_and_fence(
    database: &Database,
    name: GateName,
    agent: &Agent,
    session: &mut ClientSession,
) -> Result<Option<GateProof>, TransactionLogError> {
    let Some(gate) = read(database, name, session).await? else {
        return Ok(None);
    };
    if !gate.policy.selects(agent.id.as_str(), agent.owner.as_str()) {
        return Ok(None);
    }
    if gate.admission_fence == i64::MAX {
        return Err(SandboxStoreError::Unavailable(
            "Agent log gate admission fence exhausted".into(),
        )
        .into());
    }
    let result = database.collection::<Document>(COLLECTION).update_one(
        doc! {"_id":name.as_str(),"format_version":1_i64,"revision":gate.revision,"admission_fence":gate.admission_fence,"policy.enabled":true},
        doc! {"$inc":{"admission_fence":1_i64}},
    ).session(session).await.map_err(TransactionLogError::Driver)?;
    if result.matched_count != 1 {
        return Err(SandboxStoreError::Conflict.into());
    }
    Ok(Some(GateProof {
        name,
        revision: gate.revision,
    }))
}

/// Explicit operator CAS, never a process-startup rewrite of durable policy.
/// The caller commits this transaction with the normal unknown-outcome rules.
pub(in crate::store) async fn replace(
    database: &Database,
    name: GateName,
    expected_revision: i64,
    policy: Policy,
    session: &mut ClientSession,
) -> Result<GateRecord, TransactionLogError> {
    policy.validate()?;
    let revision = expected_revision
        .checked_add(1)
        .filter(|n| *n > 0)
        .ok_or(SandboxStoreError::Conflict)?;
    let current = read(database, name, session).await?;
    if current.as_ref().map_or(0, |gate| gate.revision) != expected_revision {
        return Err(SandboxStoreError::Conflict.into());
    }
    let next = GateRecord {
        name,
        format_version: 1,
        revision,
        admission_fence: current.as_ref().map_or(0, |gate| gate.admission_fence),
        policy,
    };
    let row = bson::serialize_to_document(&next).map_err(|_| invalid())?;
    let collection = database.collection::<Document>(COLLECTION);
    if let Some(current) = current {
        let result = collection.replace_one(
            doc! {"_id":name.as_str(),"format_version":1_i64,"revision":expected_revision,"admission_fence":current.admission_fence},
            row,
        ).session(session).await.map_err(TransactionLogError::Driver)?;
        if result.matched_count != 1 {
            return Err(SandboxStoreError::Conflict.into());
        }
    } else {
        collection
            .insert_one(row)
            .session(session)
            .await
            .map_err(|error| {
                if is_duplicate_key(&error) {
                    TransactionLogError::Domain(SandboxStoreError::Conflict)
                } else {
                    TransactionLogError::Driver(error)
                }
            })?;
    }
    Ok(next)
}

fn invalid() -> SandboxStoreError {
    SandboxStoreError::Corrupt("Agent log rollout gate has an invalid format or cohort".into())
}

#[cfg(test)]
#[path = "writer_gate_tests.rs"]
mod tests;
