//! Environment profiles, in MongoDB.
//!
//! Its own collection and its own file rather than more of the session store:
//! a profile outlives every session that used it, is queried by owner, and
//! shares no key with a session record.
//!
//! Variable values are written as stored. They are the one field a caller is
//! invited to treat as secret, so nothing here logs one — not on write, not on
//! read, and not in an error.

use futures_util::TryStreamExt;
use layer2_core::{
    store::{EnvironmentProfileStore, StoreError},
    EnvironmentProfile, EnvironmentProfileId, EnvironmentVariable, SkillId, SoftwarePackage,
    SubjectId,
};
use mongodb::bson::doc;
use mongodb::options::{IndexOptions, ReplaceOptions};
use mongodb::{Client, Collection, IndexModel};
use serde::{Deserialize, Serialize};

const PROFILES: &str = "environment_profiles";

/// How a variable is shaped on the wire.
#[derive(Debug, Serialize, Deserialize)]
struct VariableDoc {
    key: String,
    value: String,
}

/// How a package is shaped on the wire.
#[derive(Debug, Serialize, Deserialize)]
struct PackageDoc {
    name: String,
    #[serde(default)]
    version: Option<String>,
}

/// How a profile is shaped on the wire.
///
/// A distinct type from the core's model on purpose: the core owns its model
/// and this owns how it is written. `_id` carries the profile id so the
/// database enforces uniqueness rather than the application.
#[derive(Debug, Serialize, Deserialize)]
struct ProfileDoc {
    #[serde(rename = "_id")]
    id: String,
    owner: String,
    name: String,
    #[serde(default)]
    variables: Vec<VariableDoc>,
    #[serde(default)]
    packages: Vec<PackageDoc>,
    #[serde(default)]
    skills: Vec<String>,
    created_at: String,
    updated_at: String,
}

fn to_doc(p: &EnvironmentProfile) -> ProfileDoc {
    ProfileDoc {
        id: p.id.0.clone(),
        owner: p.owner.0.clone(),
        name: p.name.clone(),
        variables: p
            .variables
            .iter()
            .map(|v| VariableDoc {
                key: v.key.clone(),
                value: v.value.clone(),
            })
            .collect(),
        packages: p
            .packages
            .iter()
            .map(|s| PackageDoc {
                name: s.name.clone(),
                version: s.version.clone(),
            })
            .collect(),
        skills: p.skills.iter().map(|s| s.0.clone()).collect(),
        created_at: p.created_at.clone(),
        updated_at: p.updated_at.clone(),
    }
}

fn from_doc(d: ProfileDoc) -> EnvironmentProfile {
    EnvironmentProfile {
        id: EnvironmentProfileId(d.id),
        owner: SubjectId(d.owner),
        name: d.name,
        variables: d
            .variables
            .into_iter()
            .map(|v| EnvironmentVariable::new(v.key, v.value))
            .collect(),
        packages: d
            .packages
            .into_iter()
            .map(|p| SoftwarePackage {
                name: p.name,
                version: p.version,
            })
            .collect(),
        skills: d.skills.into_iter().map(SkillId::new).collect(),
        created_at: d.created_at,
        updated_at: d.updated_at,
    }
}

/// A MongoDB-backed environment profile store.
pub struct MongoEnvironmentProfileStore {
    profiles: Collection<ProfileDoc>,
}

impl MongoEnvironmentProfileStore {
    /// Open the store over an existing connection, ensuring its index exists.
    ///
    /// Takes a client rather than a URI so one connection serves every store
    /// this deployment keeps in the same database. See `super::connect`.
    pub(super) async fn open(client: &Client, database: &str) -> Result<Self, StoreError> {
        let store = Self {
            profiles: client.database(database).collection(PROFILES),
        };
        store.ensure_indexes().await?;
        tracing::info!(database, "environment profile store ready");
        Ok(store)
    }

    async fn ensure_indexes(&self) -> Result<(), StoreError> {
        // Every list is by owner and newest first, which is the only query
        // shape this collection ever serves.
        let owner = IndexModel::builder()
            .keys(doc! { "owner": 1, "created_at": -1 })
            .options(IndexOptions::builder().build())
            .build();
        self.profiles
            .create_index(owner)
            .await
            .map_err(|e| StoreError::Unavailable(format!("profile index: {e}")))?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl EnvironmentProfileStore for MongoEnvironmentProfileStore {
    async fn upsert(&self, profile: &EnvironmentProfile) -> Result<(), StoreError> {
        let doc = to_doc(profile);
        self.profiles
            .replace_one(doc! { "_id": &doc.id }, &doc)
            .with_options(ReplaceOptions::builder().upsert(true).build())
            .await
            .map_err(|e| StoreError::Unavailable(format!("upsert profile: {e}")))?;
        Ok(())
    }

    async fn get(
        &self,
        id: &EnvironmentProfileId,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        let found = self
            .profiles
            .find_one(doc! { "_id": &id.0 })
            .await
            .map_err(|e| StoreError::Unavailable(format!("get profile: {e}")))?;
        Ok(found.map(from_doc))
    }

    async fn list_for(&self, owner: &SubjectId) -> Result<Vec<EnvironmentProfile>, StoreError> {
        let mut cursor = self
            .profiles
            .find(doc! { "owner": &owner.0 })
            .sort(doc! { "created_at": -1 })
            .await
            .map_err(|e| StoreError::Unavailable(format!("list profiles: {e}")))?;
        let mut out = Vec::new();
        while let Some(d) = cursor
            .try_next()
            .await
            .map_err(|e| StoreError::Unavailable(format!("profile cursor: {e}")))?
        {
            out.push(from_doc(d));
        }
        Ok(out)
    }

    async fn delete(&self, id: &EnvironmentProfileId) -> Result<(), StoreError> {
        self.profiles
            .delete_one(doc! { "_id": &id.0 })
            .await
            .map_err(|e| StoreError::Unavailable(format!("delete profile: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> EnvironmentProfile {
        EnvironmentProfile {
            id: EnvironmentProfileId("envp_1".into()),
            owner: SubjectId("subject-a".into()),
            name: "build tools".into(),
            variables: vec![EnvironmentVariable::new("BUILD_MODE", "release")],
            packages: vec![
                SoftwarePackage::new("platform-cli"),
                SoftwarePackage {
                    name: "ripgrep".into(),
                    version: Some("14".into()),
                },
            ],
            skills: vec![SkillId::new("skill-one")],
            created_at: "2026-08-12T00:00:00Z".into(),
            updated_at: "2026-08-13T00:00:00Z".into(),
        }
    }

    #[test]
    fn a_profile_round_trips_through_its_stored_shape() {
        let original = profile();
        let back = from_doc(to_doc(&original));

        assert_eq!(back.id, original.id);
        assert_eq!(back.owner, original.owner);
        assert_eq!(back.name, original.name);
        assert_eq!(back.variables, original.variables);
        assert_eq!(back.packages, original.packages);
        assert_eq!(back.skills, original.skills);
        assert_eq!(back.created_at, original.created_at);
        assert_eq!(back.updated_at, original.updated_at);
    }

    #[test]
    fn the_owner_is_stored_as_a_queryable_field_not_only_inside_the_document() {
        // Listing filters on it, so it has to be a top-level field rather than
        // something only reachable by reading every record.
        let doc = to_doc(&profile());
        let json = serde_json::to_value(&doc).expect("serializes");
        assert_eq!(json["owner"], "subject-a");
        assert_eq!(json["_id"], "envp_1");
    }

    #[test]
    fn a_document_written_before_a_field_existed_still_reads() {
        // Absent lists deserialize as empty rather than failing: a profile
        // stored by an older build must not become unreadable.
        let doc: ProfileDoc = serde_json::from_str(
            r#"{"_id":"envp_2","owner":"subject-a","name":"old",
                "created_at":"2026-01-01T00:00:00Z","updated_at":"2026-01-01T00:00:00Z"}"#,
        )
        .expect("parses");
        let profile = from_doc(doc);
        assert!(profile.variables.is_empty());
        assert!(profile.packages.is_empty());
        assert!(profile.skills.is_empty());
    }

    #[test]
    fn an_unpinned_package_survives_the_round_trip_unpinned() {
        let doc = to_doc(&profile());
        let back = from_doc(doc);
        let cli = back
            .packages
            .iter()
            .find(|p| p.name == "platform-cli")
            .expect("present");
        assert!(cli.version.is_none());
    }
}
