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
    store::{EnvironmentProfileStore, Page, PageCursor, PageRequest, StoreError},
    AgentConfiguration, CredentialMode, EnvironmentProfile, EnvironmentProfileId,
    EnvironmentProfileSpec, EnvironmentVariable, KernelId, PermissionMode, ReasoningEffort,
    ServiceId, SessionMode, SkillId, SoftwarePackage, SubjectId,
};
use mongodb::bson::doc;
use mongodb::options::{IndexOptions, ReplaceOptions, ReturnDocument};
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
    /// Skills, as profiles written before the agent section stored them.
    ///
    /// Read and never written. A profile stored by an older build keeps its
    /// skills here, and dropping the field would silently empty it; migrating
    /// on read means the next save moves them without anybody being asked.
    #[serde(default)]
    skills: Vec<String>,
    #[serde(default)]
    agent: Option<AgentDoc>,
    /// Whether its owner's sessions start from this profile.
    ///
    /// Absent on every profile stored before the preference existed, which
    /// reads back as `false` — the right answer for all of them, since nobody
    /// had chosen one.
    #[serde(default)]
    preferred: bool,
    /// Platform services a session from this profile may reach.
    ///
    /// Absent on every profile stored before selections existed, which reads
    /// as "nothing chosen" — the deployment's floor still applies, so such a
    /// profile behaves exactly as it did.
    #[serde(default)]
    services: Vec<String>,
    /// Standing instructions the owner wrote for their own sessions.
    ///
    /// Defaulted so a profile stored before this existed reads back as one
    /// that adds nothing, rather than failing to read at all.
    #[serde(default)]
    agent_notes: String,
    created_at: String,
    updated_at: String,
}

/// The agent half of a stored profile.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AgentDoc {
    kernel: String,
    /// Which plane sessions from this profile are driven on.
    ///
    /// Defaulted because profiles written before modes existed named none, and
    /// every one of them was a terminal.
    #[serde(default)]
    mode: SessionMode,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    reasoning_effort: ReasoningEffort,
    #[serde(default)]
    permission: PermissionMode,
    #[serde(default)]
    credential: CredentialMode,
    /// The size sessions from this profile reserve.
    ///
    /// `None` is the honest reading of a profile that never named a size: it
    /// takes this deployment's reservation rather than an invented number.
    #[serde(default)]
    size: Option<layer2_core::SessionSize>,
    #[serde(default)]
    record_tool_bodies: bool,
    #[serde(default)]
    skills: Vec<String>,
}

/// Fields an atomic content replacement is allowed to mutate.
///
/// Server-owned fields are absent by construction, so serializing this value
/// cannot accidentally overwrite identity, ownership, creation time, or the
/// client's current preselection.
#[derive(Debug, Serialize)]
struct ProfileContentDoc {
    name: String,
    variables: Vec<VariableDoc>,
    packages: Vec<PackageDoc>,
    /// Clear the legacy location whenever current content is written.
    skills: Vec<String>,
    agent: AgentDoc,
    services: Vec<String>,
    agent_notes: String,
    updated_at: String,
}

fn variable_docs(variables: &[EnvironmentVariable]) -> Vec<VariableDoc> {
    variables
        .iter()
        .map(|variable| VariableDoc {
            key: variable.key.clone(),
            value: variable.value.clone(),
        })
        .collect()
}

fn package_docs(packages: &[SoftwarePackage]) -> Vec<PackageDoc> {
    packages
        .iter()
        .map(|package| PackageDoc {
            name: package.name.clone(),
            version: package.version.clone(),
        })
        .collect()
}

fn agent_doc(agent: &AgentConfiguration) -> AgentDoc {
    AgentDoc {
        kernel: agent.kernel.0.clone(),
        mode: agent.mode,
        model: agent.model.clone(),
        reasoning_effort: agent.reasoning_effort,
        permission: agent.permission,
        credential: agent.credential,
        size: agent.size,
        record_tool_bodies: agent.record_tool_bodies,
        skills: agent.skills.iter().map(|skill| skill.0.clone()).collect(),
    }
}

fn profile_content_doc(content: &EnvironmentProfileSpec, updated_at: &str) -> ProfileContentDoc {
    ProfileContentDoc {
        name: content.name.clone(),
        variables: variable_docs(&content.variables),
        packages: package_docs(&content.packages),
        skills: Vec::new(),
        agent: agent_doc(&content.agent),
        services: content
            .services
            .iter()
            .map(|service| service.0.clone())
            .collect(),
        agent_notes: content.agent_notes.clone(),
        updated_at: updated_at.to_string(),
    }
}

fn profile_content_update(
    content: &EnvironmentProfileSpec,
    updated_at: &str,
) -> Result<mongodb::bson::Document, StoreError> {
    let fields = mongodb::bson::serialize_to_document(&profile_content_doc(content, updated_at))
        .map_err(|_| StoreError::Malformed("profile content cannot be encoded".into()))?;
    Ok(doc! { "$set": fields })
}

fn to_doc(p: &EnvironmentProfile) -> ProfileDoc {
    ProfileDoc {
        id: p.id.0.clone(),
        owner: p.owner.0.clone(),
        name: p.name.clone(),
        variables: variable_docs(&p.variables),
        packages: package_docs(&p.packages),
        // Written only in the agent section from here on. The legacy field is
        // emptied so a document is migrated by being saved, rather than
        // carrying two copies that can disagree.
        skills: Vec::new(),
        services: p.services.iter().map(|s| s.0.clone()).collect(),
        agent_notes: p.agent_notes.clone(),
        agent: Some(agent_doc(&p.agent)),
        created_at: p.created_at.clone(),
        updated_at: p.updated_at.clone(),
        preferred: p.preferred,
    }
}

fn from_doc(d: ProfileDoc) -> EnvironmentProfile {
    let preferred = d.preferred;
    EnvironmentProfile {
        preferred,
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
        services: d.services.into_iter().map(ServiceId::new).collect(),
        agent_notes: d.agent_notes,
        // A document written before the agent section has no agent and its
        // skills at the top level. It reads as the deployment's ordinary
        // behaviour, carrying the skills it was saved with.
        agent: match d.agent {
            Some(a) => AgentConfiguration {
                kernel: KernelId::new(a.kernel),
                mode: a.mode,
                model: a.model,
                reasoning_effort: a.reasoning_effort,
                permission: a.permission,
                credential: a.credential,
                size: a.size,
                record_tool_bodies: a.record_tool_bodies,
                skills: a.skills.into_iter().map(SkillId::new).collect(),
            },
            None => AgentConfiguration {
                skills: d.skills.into_iter().map(SkillId::new).collect(),
                ..AgentConfiguration::default()
            },
        },
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
        for index in profile_indexes() {
            self.profiles
                .create_index(index)
                .await
                .map_err(|e| StoreError::Unavailable(format!("profile index: {e}")))?;
        }
        Ok(())
    }
}

fn profile_indexes() -> [IndexModel; 2] {
    [
        IndexModel::builder()
            .keys(doc! { "owner": 1, "created_at": -1, "_id": -1 })
            .options(IndexOptions::builder().build())
            .build(),
        IndexModel::builder()
            .keys(doc! { "owner": 1, "preferred": 1, "updated_at": -1, "_id": -1 })
            .options(IndexOptions::builder().build())
            .build(),
    ]
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

    async fn replace_content(
        &self,
        id: &EnvironmentProfileId,
        content: &EnvironmentProfileSpec,
        updated_at: &str,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        let update = profile_content_update(content, updated_at)?;
        self.profiles
            .find_one_and_update(doc! { "_id": &id.0 }, update)
            .return_document(ReturnDocument::After)
            .upsert(false)
            .await
            .map(|profile| profile.map(from_doc))
            .map_err(|e| StoreError::Unavailable(format!("replace profile content: {e}")))
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

    async fn list_for(
        &self,
        owner: &SubjectId,
        page: &PageRequest,
    ) -> Result<Page<EnvironmentProfile>, StoreError> {
        let mut cursor = self
            .profiles
            .find(profile_page_filter(owner, page))
            .sort(profile_page_sort())
            .limit(profile_page_fetch_limit(page))
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
        let has_more = out.len() > page.limit() as usize;
        if has_more {
            out.pop();
        }
        let next = has_more.then(|| {
            let last = out.last().expect("a non-empty bounded page has a last row");
            PageCursor::new(&last.created_at, &last.id.0)
        });
        Ok(Page { items: out, next })
    }

    async fn set_preferred(
        &self,
        owner: &SubjectId,
        chosen: Option<&EnvironmentProfileId>,
    ) -> Result<(), StoreError> {
        if let Some(id) = chosen {
            let selected = self
                .profiles
                .find_one(doc! { "_id": &id.0, "owner": &owner.0 })
                .await
                .map_err(|e| StoreError::Unavailable(format!("validate preferred profile: {e}")))?;
            if selected.is_none() {
                return Err(StoreError::Malformed(
                    "selected environment profile is not owned by the subject".into(),
                ));
            }
        }

        self.profiles
            .update_many(
                doc! { "owner": &owner.0, "preferred": true },
                doc! { "$set": { "preferred": false } },
            )
            .await
            .map_err(|e| StoreError::Unavailable(format!("clear preferred profile: {e}")))?;

        if let Some(id) = chosen {
            let result = self
                .profiles
                .update_one(
                    doc! { "_id": &id.0, "owner": &owner.0 },
                    doc! { "$set": { "preferred": true } },
                )
                .await
                .map_err(|e| StoreError::Unavailable(format!("set preferred profile: {e}")))?;
            if result.matched_count != 1 {
                return Err(StoreError::Unavailable(
                    "selected environment profile disappeared while setting preference".into(),
                ));
            }
        }
        Ok(())
    }

    async fn preferred_for(
        &self,
        owner: &SubjectId,
    ) -> Result<Option<EnvironmentProfile>, StoreError> {
        self.profiles
            .find_one(doc! { "owner": &owner.0, "preferred": true })
            .sort(doc! { "updated_at": -1, "_id": -1 })
            .await
            .map(|profile| profile.map(from_doc))
            .map_err(|e| StoreError::Unavailable(format!("preferred profile: {e}")))
    }

    async fn delete(&self, id: &EnvironmentProfileId) -> Result<(), StoreError> {
        self.profiles
            .delete_one(doc! { "_id": &id.0 })
            .await
            .map_err(|e| StoreError::Unavailable(format!("delete profile: {e}")))?;
        Ok(())
    }
}

fn profile_page_sort() -> mongodb::bson::Document {
    doc! { "created_at": -1, "_id": -1 }
}

fn profile_page_fetch_limit(page: &PageRequest) -> i64 {
    i64::from(page.limit().saturating_add(1))
}

fn profile_page_filter(owner: &SubjectId, page: &PageRequest) -> mongodb::bson::Document {
    let mut filter = doc! { "owner": &owner.0 };
    if let Some(cursor) = page.cursor() {
        filter.insert(
            "$or",
            vec![
                doc! { "created_at": { "$lt": &cursor.created_at } },
                doc! {
                    "created_at": &cursor.created_at,
                    "_id": { "$lt": &cursor.id },
                },
            ],
        );
    }
    filter
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> EnvironmentProfile {
        EnvironmentProfile {
            preferred: false,
            services: Vec::new(),
            agent_notes: String::new(),
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
            agent: AgentConfiguration {
                skills: vec![SkillId::new("skill-one")],
                ..AgentConfiguration::default()
            },
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
        assert_eq!(back.agent.skills, original.agent.skills);
        assert_eq!(back.created_at, original.created_at);
        assert_eq!(back.updated_at, original.updated_at);
    }

    #[test]
    fn a_profile_size_round_trips_through_bson() {
        let mut original = profile();
        original.agent.size = Some(layer2_core::SessionSize::new(1_500, 3_072).unwrap());

        let encoded = mongodb::bson::serialize_to_bson(&to_doc(&original)).expect("serializes");
        let decoded: ProfileDoc =
            mongodb::bson::deserialize_from_bson(encoded).expect("deserializes");
        let back = from_doc(decoded);

        assert_eq!(back.agent.size, original.agent.size);
    }

    #[test]
    fn a_bson_profile_without_a_size_reads_as_the_deployments_own() {
        let encoded = mongodb::bson::Bson::Document(doc! {
            "_id": "envp_old",
            "owner": "subject-a",
            "name": "old",
            "agent": { "kernel": "fake" },
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
        });
        let decoded: ProfileDoc =
            mongodb::bson::deserialize_from_bson(encoded).expect("an old profile reads");

        assert_eq!(from_doc(decoded).agent.size, None);
    }

    #[test]
    fn a_bson_profile_with_a_zero_size_is_refused() {
        let encoded = mongodb::bson::Bson::Document(doc! {
            "_id": "envp_bad",
            "owner": "subject-a",
            "name": "bad",
            "agent": {
                "kernel": "fake",
                "size": { "cpu_millis": 0, "memory_mib": 512 },
            },
            "created_at": "2026-01-01T00:00:00Z",
            "updated_at": "2026-01-01T00:00:00Z",
        });

        assert!(
            mongodb::bson::deserialize_from_bson::<ProfileDoc>(encoded).is_err(),
            "a stored zero must not become a usable session size"
        );
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
    fn a_content_update_cannot_name_any_server_owned_field() {
        let mut content = profile().to_spec();
        content.name = "replacement".into();
        let update =
            profile_content_update(&content, "2026-08-25T10:00:00Z").expect("content serializes");
        let fields = update.get_document("$set").expect("an atomic set document");

        for protected in ["_id", "owner", "created_at", "preferred"] {
            assert!(
                !fields.contains_key(protected),
                "content replacement must not write {protected}"
            );
        }
        assert_eq!(fields.get_str("name").unwrap(), "replacement");
        assert_eq!(
            fields.get_str("updated_at").unwrap(),
            "2026-08-25T10:00:00Z"
        );
        assert!(fields.get_array("skills").unwrap().is_empty());
        assert_eq!(
            fields
                .get_document("agent")
                .unwrap()
                .get_array("skills")
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    #[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at MongoDB"]
    async fn mongo_content_replacement_preserves_preference_and_never_upserts() {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI")
            .expect("CHRONO_TEST_MONGODB_URI must name MongoDB");
        let client = Client::with_uri_str(uri)
            .await
            .expect("test Mongo connects");
        let unique = EnvironmentProfileId::generate();
        let database = format!("chrono_profile_replace_{}", unique.as_str());
        let store = MongoEnvironmentProfileStore::open(&client, &database)
            .await
            .expect("profile store opens");
        let mut original = profile();
        original.id = unique.clone();
        store.upsert(&original).await.expect("profile is inserted");

        let mut content = original.to_spec();
        content.name = "replacement after set".into();
        store
            .set_preferred(&original.owner, Some(&original.id))
            .await
            .expect("preference is set concurrently");
        let after_set = store
            .replace_content(&original.id, &content, "2026-08-25T10:00:00Z")
            .await
            .expect("content replacement succeeds")
            .expect("row still exists");
        assert!(after_set.preferred);
        assert_eq!(after_set.owner, original.owner);
        assert_eq!(after_set.created_at, original.created_at);

        content.name = "replacement after clear".into();
        store
            .set_preferred(&original.owner, None)
            .await
            .expect("preference is cleared concurrently");
        let after_clear = store
            .replace_content(&original.id, &content, "2026-08-25T11:00:00Z")
            .await
            .expect("content replacement succeeds")
            .expect("row still exists");
        assert!(!after_clear.preferred);

        store.delete(&original.id).await.expect("row is deleted");
        assert!(store
            .replace_content(&original.id, &content, "2026-08-25T12:00:00Z")
            .await
            .expect("absence is not a store failure")
            .is_none());
        assert!(store
            .get(&original.id)
            .await
            .expect("store reads")
            .is_none());

        client
            .database(&database)
            .drop()
            .await
            .expect("test database is removed");
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
        assert!(profile.agent.skills.is_empty());
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

    #[test]
    fn profile_pages_fetch_one_sentinel_row_and_never_exceed_the_store_bound() {
        let five = PageRequest::new(5, None);
        let oversized = PageRequest::new(u32::MAX, None);

        assert_eq!(profile_page_fetch_limit(&five), 6);
        assert_eq!(oversized.limit(), 200);
        assert_eq!(profile_page_fetch_limit(&oversized), 201);
    }

    #[test]
    fn profile_page_filter_uses_the_timestamp_and_id_tie_breaker() {
        let page = PageRequest::new(
            50,
            Some(PageCursor::new("2026-08-24T10:11:12Z", "envp_tied")),
        );

        assert_eq!(
            profile_page_filter(&SubjectId("subject-a".into()), &page),
            doc! {
                "owner": "subject-a",
                "$or": [
                    { "created_at": { "$lt": "2026-08-24T10:11:12Z" } },
                    {
                        "created_at": "2026-08-24T10:11:12Z",
                        "_id": { "$lt": "envp_tied" },
                    },
                ],
            }
        );
        assert_eq!(profile_page_sort(), doc! { "created_at": -1, "_id": -1 });
        assert_eq!(
            profile_indexes()[0].keys,
            doc! { "owner": 1, "created_at": -1, "_id": -1 }
        );
    }
}
