//! Optional draft pins preserve unresolved-reference compatibility.
use layer2_core::{
    profiles::{SandboxProfile, WorkspaceProfile},
    trigger_domain::TriggerTarget,
    PlatformDefaults,
};
use mongodb::{
    bson::{doc, Document},
    ClientSession, Database,
};
use std::collections::BTreeSet;

use crate::store::{durability::majority_collection, profile_pin_admission};

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(in crate::store) struct Pin {
    kind: Kind,
    id: String,
    revision: u32,
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Kind {
    Agent,
    Workspace,
    Sandbox,
}

impl Pin {
    pub(in crate::store) fn agent(id: &str, revision: u32) -> Self {
        Self {
            kind: Kind::Agent,
            id: id.into(),
            revision,
        }
    }
    pub(in crate::store) fn workspace(id: &str, revision: u32) -> Self {
        Self {
            kind: Kind::Workspace,
            id: id.into(),
            revision,
        }
    }
    pub(in crate::store) fn sandbox(id: &str, revision: u32) -> Self {
        Self {
            kind: Kind::Sandbox,
            id: id.into(),
            revision,
        }
    }
    fn aggregate(&self) -> &'static str {
        match self.kind {
            Kind::Agent => "agent_profiles",
            Kind::Workspace => "workspace_profiles",
            Kind::Sandbox => "sandbox_profiles",
        }
    }
    fn revisions(&self) -> &'static str {
        match self.kind {
            Kind::Agent => "agent_profile_revisions",
            Kind::Workspace => "workspace_profile_revisions",
            Kind::Sandbox => "sandbox_profile_revisions",
        }
    }
    async fn available(
        &self,
        db: &Database,
        session: &mut ClientSession,
    ) -> mongodb::error::Result<bool> {
        match self.kind {
            Kind::Agent => {
                profile_pin_admission::agent(db, &self.id, self.revision, None, session).await
            }
            Kind::Workspace => {
                profile_pin_admission::workspace(db, &self.id, self.revision, None, session).await
            }
            Kind::Sandbox => {
                profile_pin_admission::sandbox(db, &self.id, self.revision, None, session).await
            }
        }
    }
}

#[derive(Default)]
pub(in crate::store) struct Pins {
    references: BTreeSet<Pin>,
    required: BTreeSet<Pin>,
    guards: Vec<(&'static str, Document)>,
}

impl Pins {
    pub(in crate::store) fn workspace(profile: &WorkspaceProfile) -> Self {
        Self {
            references: profile
                .draft
                .agent_profiles
                .iter()
                .map(|pin| Pin::agent(pin.id.as_str(), pin.revision))
                .collect(),
            ..Self::default()
        }
    }
    pub(in crate::store) fn sandbox(profile: &SandboxProfile) -> Self {
        Self {
            references: profile
                .draft
                .workspace_profiles
                .iter()
                .map(|pin| Pin::workspace(pin.id.as_str(), pin.revision))
                .collect(),
            ..Self::default()
        }
    }
    pub(in crate::store) fn trigger(target: &TriggerTarget) -> Self {
        Self {
            references: BTreeSet::from([
                Pin::agent(
                    target.agent_profile.id.as_str(),
                    target.agent_profile.revision,
                ),
                Pin::workspace(
                    target.workspace_profile.id.as_str(),
                    target.workspace_profile.revision,
                ),
                Pin::sandbox(
                    target.sandbox_profile.id.as_str(),
                    target.sandbox_profile.revision,
                ),
            ]),
            ..Self::default()
        }
    }
    pub(in crate::store) fn defaults(defaults: &PlatformDefaults) -> Self {
        Self::trigger(&TriggerTarget {
            agent_profile: defaults.agent_profile.clone(),
            workspace_profile: defaults.workspace_profile.clone(),
            sandbox_profile: defaults.sandbox_profile.clone(),
            inline: None,
        })
    }
    pub(in crate::store) fn strict(pin: Pin) -> Self {
        Self {
            references: BTreeSet::from([pin.clone()]),
            required: BTreeSet::from([pin]),
            ..Self::default()
        }
    }
    pub(in crate::store) fn guard(&mut self, collection: &'static str, filter: Document) {
        self.guards.push((collection, filter));
    }
    pub(super) async fn admit(
        &mut self,
        db: &Database,
        session: &mut ClientSession,
    ) -> mongodb::error::Result<bool> {
        for (collection, filter) in &self.guards {
            if majority_collection::<Document>(db, collection)
                .find_one(filter.clone())
                .session(&mut *session)
                .await?
                .is_none()
            {
                return Ok(false);
            }
        }
        for pin in &self.references {
            let aggregate = majority_collection::<Document>(db, pin.aggregate())
                .find_one(doc! {"_id":&pin.id})
                .projection(doc! {"account_erasure_fenced":1})
                .session(&mut *session)
                .await?;
            // Never downgrade a fenced aggregate to an unresolved placeholder
            // merely because erasure has already removed its exact revision.
            if aggregate
                .as_ref()
                .is_some_and(|row| row.get_bool("account_erasure_fenced").ok() == Some(true))
            {
                return Ok(false);
            }
            let known = self.required.contains(pin)
                || (aggregate.is_some()
                    && majority_collection::<Document>(db, pin.revisions())
                        .find_one(doc! {"profile_id":&pin.id,"revision":i64::from(pin.revision)})
                        .projection(doc! {"_id":1})
                        .session(&mut *session)
                        .await?
                        .is_some());
            if known {
                self.required.insert(pin.clone());
                if !pin.available(db, session).await? {
                    return Ok(false);
                }
            }
        }
        Ok(true)
    }
}
