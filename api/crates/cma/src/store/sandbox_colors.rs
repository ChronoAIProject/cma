//! One majority-durable colour palette per owner, beside the Sandbox records.
//!
//! The palette is its own collection because the Sandbox record is read with
//! unknown fields denied: a colour stored inside it would make a rolled-back
//! reader refuse every coloured Sandbox, while this collection is simply
//! ignored by a reader that predates it.

use std::collections::BTreeMap;

use async_trait::async_trait;
use layer2_core::sandbox::colors::{OwnerSandboxColors, SandboxColor, SandboxColorStore};
use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::{SandboxId, StoreError, SubjectId};
use mongodb::{bson::doc, Collection, Database};
use serde::{Deserialize, Serialize};

use super::{durability::majority_collection, schema};

pub(super) const COLLECTION: &str = "sandbox_colors";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Assignment {
    sandbox_id: String,
    color: SandboxColor,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Palette {
    /// The owner's subject: one palette per owner makes every assignment a
    /// single-document compare-and-set.
    #[serde(rename = "_id")]
    owner: String,
    document_schema_version: i64,
    revision: i64,
    colors: Vec<Assignment>,
}

pub(crate) struct MongoSandboxColorStore {
    palettes: Collection<Palette>,
}

impl MongoSandboxColorStore {
    pub(crate) async fn open(database: &Database) -> Result<Self, StoreError> {
        schema::initialize_collections(database, &[COLLECTION]).await?;
        Ok(Self {
            palettes: majority_collection(database, COLLECTION),
        })
    }
}

#[async_trait]
impl SandboxColorStore for MongoSandboxColorStore {
    async fn owner_colors(
        &self,
        owner: &SubjectId,
    ) -> Result<OwnerSandboxColors, SandboxStoreError> {
        let Some(palette) = self
            .palettes
            .find_one(doc! {"_id": owner.as_str()})
            .await
            .map_err(unavailable)?
        else {
            return Ok(OwnerSandboxColors::default());
        };
        read(owner, palette)
    }

    async fn record(
        &self,
        owner: &SubjectId,
        expected_revision: u64,
        assignments: &[(SandboxId, SandboxColor)],
    ) -> Result<(), SandboxStoreError> {
        let rows: Vec<Assignment> = assignments
            .iter()
            .map(|(sandbox, color)| Assignment {
                sandbox_id: sandbox.to_string(),
                color: *color,
            })
            .collect();
        if expected_revision == 0 {
            let palette = Palette {
                owner: owner.to_string(),
                document_schema_version: schema::DOCUMENT_SCHEMA_VERSION,
                revision: 1,
                colors: rows,
            };
            return match self.palettes.insert_one(&palette).await {
                Ok(_) => Ok(()),
                // Another reader created this owner's palette first.
                Err(error) if super::sandbox_mongo::is_duplicate_key(&error) => {
                    Err(SandboxStoreError::Conflict)
                }
                Err(error) => Err(unavailable(error)),
            };
        }
        let expected = i64::try_from(expected_revision).map_err(|_| corrupt("revision"))?;
        let next = expected.checked_add(1).ok_or_else(|| corrupt("revision"))?;
        let ids: Vec<&str> = rows.iter().map(|row| row.sandbox_id.as_str()).collect();
        let rows = mongodb::bson::serialize_to_bson(&rows).map_err(|_| corrupt("assignment"))?;
        let result = self
            .palettes
            .update_one(
                doc! {
                    "_id": owner.as_str(),
                    "revision": expected,
                    "colors.sandbox_id": {"$nin": ids},
                },
                doc! {"$set": {"revision": next}, "$push": {"colors": {"$each": rows}}},
            )
            .await
            .map_err(unavailable)?;
        if result.matched_count == 0 {
            return Err(SandboxStoreError::Conflict);
        }
        Ok(())
    }
}

fn read(owner: &SubjectId, palette: Palette) -> Result<OwnerSandboxColors, SandboxStoreError> {
    schema::require_current(palette.document_schema_version)
        .map_err(|_| corrupt("schema version"))?;
    let revision = u64::try_from(palette.revision)
        .ok()
        .filter(|revision| *revision > 0)
        .ok_or_else(|| corrupt("revision"))?;
    if palette.owner != owner.as_str() {
        return Err(corrupt("owner"));
    }
    let mut colors = BTreeMap::new();
    for row in palette.colors {
        if colors
            .insert(SandboxId(row.sandbox_id), row.color)
            .is_some()
        {
            return Err(corrupt("duplicate sandbox"));
        }
    }
    Ok(OwnerSandboxColors { revision, colors })
}

fn unavailable(_: mongodb::error::Error) -> SandboxStoreError {
    SandboxStoreError::Unavailable("sandbox colour storage unavailable".into())
}

fn corrupt(field: &str) -> SandboxStoreError {
    SandboxStoreError::Corrupt(format!("sandbox colour palette has an invalid {field}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn palette(owner: &str, revision: i64, colors: &[(&str, &str)]) -> Palette {
        Palette {
            owner: owner.into(),
            document_schema_version: schema::DOCUMENT_SCHEMA_VERSION,
            revision,
            colors: colors
                .iter()
                .map(|(sandbox, color)| Assignment {
                    sandbox_id: (*sandbox).into(),
                    color: SandboxColor::parse(color).unwrap(),
                })
                .collect(),
        }
    }

    #[test]
    fn stored_palettes_are_checked_before_use() {
        let owner = SubjectId::new("owner");
        let read_back = read(&owner, palette("owner", 2, &[("sbx_a", "#2f96fa")])).unwrap();
        assert_eq!(read_back.revision, 2);
        assert_eq!(
            read_back.colors.get(&SandboxId("sbx_a".into())),
            Some(&SandboxColor::parse("#2f96fa").unwrap())
        );
        for invalid in [
            palette("other", 1, &[]),
            palette("owner", 0, &[]),
            palette("owner", 1, &[("sbx_a", "#2f96fa"), ("sbx_a", "#62d079")]),
            Palette {
                document_schema_version: schema::DOCUMENT_SCHEMA_VERSION + 1,
                ..palette("owner", 1, &[])
            },
        ] {
            assert!(matches!(
                read(&owner, invalid),
                Err(SandboxStoreError::Corrupt(_))
            ));
        }
    }

    #[test]
    fn stored_colours_keep_their_single_spelling() {
        let stored =
            mongodb::bson::serialize_to_document(&palette("owner", 1, &[("sbx_a", "#2f96fa")]))
                .unwrap();
        let colors = stored.get_array("colors").unwrap();
        let first = colors[0].as_document().unwrap();
        assert_eq!(first.get_str("color").unwrap(), "#2f96fa");
        let mut tampered = first.clone();
        tampered.insert("color", "#2F96FA");
        assert!(mongodb::bson::deserialize_from_document::<Assignment>(tampered).is_err());
    }

    #[tokio::test]
    #[ignore = "requires disposable Mongo fixture"]
    async fn palettes_survive_reopen_compare_and_set_and_isolate_owners() {
        let uri = std::env::var("CHRONO_TEST_MONGODB_URI").expect("disposable Mongo URI");
        let client = mongodb::Client::with_uri_str(uri).await.unwrap();
        let database = client.database(&format!("cma_colors_{}", uuid::Uuid::new_v4().simple()));
        let owner = SubjectId::new("colour-owner");
        let other = SubjectId::new("colour-other");
        let blue = SandboxColor::parse("#2f96fa").unwrap();
        let green = SandboxColor::parse("#62d079").unwrap();
        let first = SandboxId("sbx_first".into());
        let second = SandboxId("sbx_second".into());
        let store = MongoSandboxColorStore::open(&database).await.unwrap();
        store
            .record(&owner, 0, &[(first.clone(), blue)])
            .await
            .unwrap();
        assert_eq!(
            store.record(&owner, 0, &[(second.clone(), green)]).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(
            store.record(&owner, 2, &[(second.clone(), green)]).await,
            Err(SandboxStoreError::Conflict)
        );
        assert_eq!(
            store.record(&owner, 1, &[(first.clone(), green)]).await,
            Err(SandboxStoreError::Conflict)
        );
        store
            .record(&owner, 1, &[(second.clone(), green)])
            .await
            .unwrap();
        drop(store);

        let store = MongoSandboxColorStore::open(&database).await.unwrap();
        let palette = store.owner_colors(&owner).await.unwrap();
        assert_eq!(palette.revision, 2);
        assert_eq!(palette.colors.get(&first), Some(&blue));
        assert_eq!(palette.colors.get(&second), Some(&green));
        assert_eq!(
            store.owner_colors(&other).await.unwrap(),
            OwnerSandboxColors::default()
        );
        database.drop().await.unwrap();
    }
}
