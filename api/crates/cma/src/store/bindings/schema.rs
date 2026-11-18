//! The binding index contract shared by startup inspection and explicit migration.

use mongodb::bson::doc;

use super::CONVERSATIONS;
pub(in crate::store) mod indexes_v1;
pub(in crate::store) use indexes_v1::{indexes, COLLECTIONS, LEGACY_AGENT_INDEX};

#[derive(Clone, Copy, Debug, Default)]
pub(in crate::store) struct BindingSafety {
    pub(super) keys: bool,
    pub(super) associations: bool,
    pub(super) preparations: bool,
}

impl BindingSafety {
    pub(in crate::store) fn require_keys(
        self,
    ) -> Result<(), layer2_core::sandbox::store::SandboxStoreError> {
        Self::require(self.keys)
    }
    pub(in crate::store) fn require_associations(
        self,
    ) -> Result<(), layer2_core::sandbox::store::SandboxStoreError> {
        Self::require(self.associations)
    }
    pub(in crate::store) fn require_preparations(
        self,
    ) -> Result<(), layer2_core::sandbox::store::SandboxStoreError> {
        Self::require(self.preparations)
    }
    fn require(verified: bool) -> Result<(), layer2_core::sandbox::store::SandboxStoreError> {
        if verified {
            Ok(())
        } else {
            Err(layer2_core::sandbox::store::SandboxStoreError::Unavailable(
                "binding uniqueness is unverified; prepare schema and restart".into(),
            ))
        }
    }
}

pub(super) async fn inspect(
    database: &mongodb::Database,
) -> Result<crate::store::schema::Assessment<BindingSafety>, crate::store::schema::SchemaFinding> {
    use crate::store::schema::{inspect_collection, Assessment, SchemaFinding as Finding};
    let mut assessment = Assessment::<BindingSafety>::default();
    for &collection in COLLECTIONS {
        let existing = match inspect_collection(database, collection).await {
            Ok(Some(existing)) => existing,
            Ok(None) => {
                assessment.findings.insert(Finding::MissingCollection);
                continue;
            }
            Err(finding) => {
                assessment.findings.insert(finding);
                continue;
            }
        };
        if collection == CONVERSATIONS
            && existing.iter().any(|index| {
                index.get_str("name").ok() == Some(LEGACY_AGENT_INDEX)
                    || index.get_document("key").ok() == Some(&doc! {"record.agent_id":1})
            })
        {
            assessment.findings.insert(Finding::LegacyBindingIndex);
        }
        for desired in indexes(collection) {
            let unique = desired.get_bool("unique").unwrap_or(false);
            match crate::store::schema::index_contract_v1::missing(&existing, &desired) {
                Ok(pending) if pending.is_empty() => match desired.get_str("name").ok() {
                    Some("record.key_sha256_1") => assessment.capability.keys = true,
                    Some("binding_generation_agent") => assessment.capability.associations = true,
                    Some("binding_id_1_generation_1") => assessment.capability.preparations = true,
                    _ => {}
                },
                Ok(_) => {
                    assessment.findings.insert(if unique {
                        Finding::MissingUniqueIndex
                    } else {
                        Finding::MissingQueryIndex
                    });
                }
                Err(_) => {
                    assessment.findings.insert(Finding::DefinitionMismatch);
                }
            }
        }
    }
    Ok(assessment)
}
