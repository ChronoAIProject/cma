//! Service-owned, explicit migration execution. No runtime activation.
use std::time::Duration;

use mongodb::{
    bson::{doc, Bson, DateTime, Document},
    options::{Acknowledgment, ReadConcern, ReturnDocument, WriteConcern},
    Collection, Database,
};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

mod effects;
mod ledger;
mod reconcile;
mod schema;
mod startup;
mod transaction;
pub use effects::{command, effect};
pub use schema::{
    absent, collection, collection_with_guarded_replacement, collection_with_predecessors,
    describe, describing, exact_indexes, indexes, inspect, inspecting, marker, marker_history,
    marker_int32, marker_with_fields, update,
};

pub type Result<T> = std::result::Result<T, &'static str>;
const IO: Duration = Duration::from_secs(5);
const LEASE_MS: i64 = 30_000;
const SCHEMA: i32 = 1;

#[derive(Clone, Copy)]
pub struct Manifest {
    pub id: &'static str,
    pub prerequisites: &'static [&'static str],
    pub definition: &'static str,
    pub schema_fingerprint: [u8; 32],
}
impl Manifest {
    pub fn checksum(self) -> String {
        let mut digest = Sha256::new();
        for field in std::iter::once(self.id)
            .chain(self.prerequisites.iter().copied())
            .chain(std::iter::once(self.definition))
        {
            digest.update((field.len() as u64).to_be_bytes());
            digest.update(field.as_bytes());
        }
        digest.update(self.schema_fingerprint);
        format!("{:x}", digest.finalize())
    }
}

pub struct Release<'a> {
    pub source: &'a str,
    pub image: &'a str,
}
impl Release<'_> {
    pub fn validate(&self, service: &str) -> Result<()> {
        fn hex(value: &str, len: usize) -> bool {
            value.len() == len
                && value
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        }
        if !hex(self.source, 40)
            || !self
                .image
                .split_once("@sha256:")
                .is_some_and(|(repo, digest)| {
                    repo.rsplit('/').next() == Some(service) && hex(digest, 64)
                })
        {
            return Err("migration requires the service's immutable release identity");
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct Runner {
    db: Database,
    rows: Collection<Document>,
    service: &'static str,
    manifests: Vec<Manifest>,
}
impl Runner {
    pub fn new(
        db: &Database,
        collection: &'static str,
        service: &'static str,
        manifests: &[Manifest],
    ) -> Self {
        let db = db.client().database_with_options(
            db.name(),
            mongodb::options::DatabaseOptions::builder()
                .read_concern(ReadConcern::majority())
                .write_concern(WriteConcern::builder().w(Acknowledgment::Majority).build())
                .selection_criteria(mongodb::options::SelectionCriteria::ReadPreference(
                    mongodb::options::ReadPreference::Primary,
                ))
                .build(),
        );
        Self {
            rows: db.collection(collection),
            db,
            service,
            manifests: manifests.to_vec(),
        }
    }
    pub fn manifest(&self, id: &str) -> Result<Manifest> {
        self.manifests
            .iter()
            .find(|m| m.id == id)
            .copied()
            .ok_or("unknown migration")
    }
    fn verify_manifest(&self, manifest: Manifest) -> Result<()> {
        if self.manifest(manifest.id)?.checksum() != manifest.checksum() {
            return Err("manifest definition drift");
        }
        Ok(())
    }
    pub async fn status(&self, manifest: Manifest) -> Result<Value> {
        self.verify_manifest(manifest)?;
        self.preflight().await?;
        let completed = self.completed(manifest).await?;
        let lock = self
            .rows
            .find_one(doc! {"_id":"runner"})
            .max_time(IO)
            .await
            .map_err(database_error)?;
        let latest = self
            .rows
            .find_one(
                doc! {"kind":"attempt","migration":manifest.id,"checksum":manifest.checksum()},
            )
            .sort(doc! {"fence":-1})
            .max_time(IO)
            .await
            .map_err(database_error)?;
        Ok(
            json!({"service":self.service,"migration":manifest.id,"checksum":manifest.checksum(),"completed":completed,
            "active":lock.as_ref().and_then(|row|row.get_str("active").ok()),"strict":lock.as_ref().and_then(|row|row.get_bool("strict").ok()).unwrap_or(false),"latest_attempt":latest,"pending_effect":self.pending_effect().await?,"writes":false}),
        )
    }
    pub async fn plan(&self, manifest: Manifest, ready: bool) -> Result<Value> {
        self.verify_manifest(manifest)?;
        self.preflight().await?;
        Ok(
            json!({"service":self.service,"migration":manifest.id,"checksum":manifest.checksum(),"prerequisites":manifest.prerequisites,"definition":manifest.definition,
            "postcondition_ready":ready,"writes":false}),
        )
    }
    /// All new and legacy service-local DDL commands must use this boundary.
    /// The caller supplies deterministic schema convergence and a read-only inspection.
    pub async fn apply<F, Fut, I, Inspect>(
        &self,
        manifest: Manifest,
        checksum: &str,
        release: Release<'_>,
        inspect: I,
        apply: F,
    ) -> Result<Value>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = Result<()>>,
        I: Fn() -> Inspect,
        Inspect: std::future::Future<Output = Result<bool>>,
    {
        self.verify_manifest(manifest)?;
        release.validate(self.service)?;
        if checksum != manifest.checksum() {
            return Err("manifest checksum mismatch");
        }
        self.preflight().await?;
        let ready = inspect().await?;
        if self.completed(manifest).await? {
            if !ready {
                return Err("completed migration postcondition drift");
            }
            return Ok(json!({"migration":manifest.id,"outcome":"already_completed"}));
        }
        self.initialize().await?;
        let lease = self.claim(manifest, &release, false).await?;
        // The entire immutable convergence operation is recorded before dispatch.
        // An interrupted effect keeps its manifest pinned and requires a drained
        // prior execution before a new holder can issue DDL.
        let result = async {
            self.renew(&lease).await?;
            if !inspect().await? {
                effects::scope(self.clone(), lease.clone(), apply()).await?;
            }
            self.renew(&lease).await?;
            if !inspect().await? {
                return Err("migration postcondition unavailable");
            }
            Ok(())
        }
        .await;
        self.finish(&lease, result.is_ok(), false).await?;
        result?;
        Ok(json!({"migration":manifest.id,"outcome":"completed"}))
    }
    /// The compatibility command uses the same lock before activation. Its
    /// receipts carry explicit legacy provenance and do not adopt a manifest.
    pub async fn legacy<Fut, I, Inspect>(
        &self,
        manifest: Manifest,
        inspect: I,
        apply: Fut,
    ) -> Result<()>
    where
        Fut: std::future::Future<Output = Result<()>>,
        I: Fn() -> Inspect,
        Inspect: std::future::Future<Output = Result<bool>>,
    {
        self.verify_manifest(manifest)?;
        self.initialize().await?;
        let release = Release {
            source: "legacy",
            image: "legacy",
        };
        let lease = self.claim(manifest, &release, true).await?;
        self.execute_legacy(lease, inspect, apply).await
    }
    async fn execute_legacy<Fut, I, Inspect>(
        &self,
        lease: ledger::Lease,
        inspect: I,
        apply: Fut,
    ) -> Result<()>
    where
        Fut: std::future::Future<Output = Result<()>>,
        I: Fn() -> Inspect,
        Inspect: std::future::Future<Output = Result<bool>>,
    {
        let result = async {
            if !inspect().await? {
                effects::scope(self.clone(), lease.clone(), apply).await?;
            }
            self.renew(&lease).await?;
            if !inspect().await? {
                return Err("migration postcondition unavailable");
            }
            Ok(())
        }
        .await;
        self.finish(&lease, result.is_ok(), true).await?;
        result
    }
}

fn database_error(_: mongodb::error::Error) -> &'static str {
    "migration database operation unavailable"
}

pub fn migration_role(mut role: Document, database: &str, collection: &str) -> Document {
    let grants = role
        .get_array_mut("privileges")
        .expect("migration role privileges");
    for grant in [
        doc! {"resource":{"db":database,"collection":collection},"actions":["find","insert","update","createCollection"]},
        doc! {"resource":{"db":database,"collection":""},"actions":["listCollections"]},
    ] {
        let grant = Bson::Document(grant);
        if !grants.contains(&grant) {
            grants.push(grant);
        }
    }
    role
}

pub fn operator_role(database: &str, collection: &str) -> Document {
    doc! {"createRole":format!("{collection}_operator"),"privileges":[{"resource":{"db":database,"collection":collection},"actions":["find","insert","update","createCollection"]}],"roles":[]}
}

#[cfg(test)]
mod tests;
