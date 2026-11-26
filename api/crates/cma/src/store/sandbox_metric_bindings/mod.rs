//! Retained independent physical compute/home assignment intervals.
use super::usage_reports::{decode, encode, expires, unavailable, ReportingStore, DB_DEADLINE};
use crate::store::transaction_scope as tx;
use cma_am_reporting::metric_bindings;
use futures_util::TryStreamExt;
use layer2_core::sandbox_metrics::{
    Binding, BindingKind, MetricsError, Range, Result, IDENTITY_RETENTION_SECONDS,
};
pub(crate) use metric_bindings::COLLECTION;
use mongodb::{
    bson::{doc, Document},
    options::IndexOptions,
    IndexModel,
};

pub(crate) async fn initialize(store: &ReportingStore) -> Result<()> {
    store
        .index(COLLECTION, doc! {"valid_from":1}, false, false)
        .await?;
    store
        .index(
            COLLECTION,
            doc! {"sandbox_id":1,"valid_from":1},
            false,
            false,
        )
        .await?;
    store
        .index(
            COLLECTION,
            doc! {"physical_key":1,"valid_from":1},
            false,
            false,
        )
        .await?;
    store
        .collection(COLLECTION)
        .create_index(
            IndexModel::builder()
                .keys(doc! {"active_physical":1})
                .options(IndexOptions::builder().unique(true).sparse(true).build())
                .build(),
        )
        .await
        .map_err(unavailable)?;
    Ok(())
}
impl ReportingStore {
    pub(crate) async fn binding_sandbox_page(
        &self,
        after: Option<&str>,
        range: Range,
    ) -> Result<Vec<String>> {
        metric_bindings::read_sandbox_page(self.collection(COLLECTION), after, range, DB_DEADLINE)
            .await
    }
    #[cfg(test)]
    pub(crate) async fn reconcile_bindings(
        &self,
        holder: &str,
        sandbox: &str,
        observed: Vec<Binding>,
        at: u64,
    ) -> Result<()> {
        self.reconcile_bindings_with_evidence(holder, sandbox, observed, at, None)
            .await
    }

    pub(crate) async fn reconcile_bindings_with_evidence(
        &self,
        holder: &str,
        sandbox: &str,
        observed: Vec<Binding>,
        at: u64,
        evidence_parent: Option<&layer2_core::sandbox::Sandbox>,
    ) -> Result<()> {
        // The original scheduling claim and mappings commit together. Unique
        // physical reservation and row CAS still reject contradictory owners.
        tx::run(
            &self.client,
            (self, holder, sandbox, &observed, at, evidence_parent),
            |&(store, holder, sandbox, observed, at, evidence_parent), session| {
                Box::pin(async move {
                    if let Some(parent) = evidence_parent {
                        let matched = store.collection("sandboxes").update_one(
                            doc! {"_id":sandbox,"owner":parent.owner.as_str(),"revision":parent.revision as i64},
                            doc! {"$inc":{"measured_observation_fence_v1":1_i64}})
                            .session(&mut *session).await.map_err(tx::mongo)?.matched_count;
                        if matched != 1 { return Err(tx::domain(MetricsError::Unavailable)); }
                    }
                    for kind in [BindingKind::Compute, BindingKind::Home] {
                        let kind_value = encode(&kind).map_err(tx::domain)?;
                        let filter =
                            doc! {"sandbox_id":sandbox,"kind":kind_value,"valid_until":null};
                        let old = store
                            .collection(COLLECTION)
                            .find_one(filter)
                            .session(&mut *session)
                            .await
                            .map_err(tx::mongo)?;
                        let previous = old
                            .as_ref()
                            .map(decode::<Binding>)
                            .transpose()
                            .map_err(tx::domain)?;
                        let candidate = observed.iter().find(|b| b.kind == kind);
                        let unchanged = previous.as_ref().zip(candidate).is_some_and(|(a, b)| {
                            same_assignment(a, b) && at.saturating_sub(a.observed_at) <= 90
                        });
                        if let Some(mut previous) = previous {
                            let expected = previous.revision;
                            previous.revision += 1;
                            if unchanged {
                                previous.observed_at = at;
                            } else {
                                previous.valid_until = Some(previous.observed_at);
                            }
                            let result = store
                                .collection(COLLECTION)
                                .replace_one(
                                    doc! {"_id":&previous.id,"revision":expected as i64},
                                    document(&previous).map_err(tx::domain)?,
                                )
                                .session(&mut *session)
                                .await
                                .map_err(tx::mongo)?;
                            if unchanged {
                                if let Some(parent) = evidence_parent {
                                    super::sandbox_usage_export::retain_binding_bridge(store, session, &previous, parent, at).await?;
                                }
                            }
                            if result.matched_count != 1 {
                                return Err(tx::domain(MetricsError::Unavailable));
                            }
                        }
                        if !unchanged && (old.is_some() || candidate.is_some()) {
                            if let Some(candidate) = candidate {
                                let mut b = candidate.clone();
                                b.id = format!("mbi_{}", uuid::Uuid::new_v4().simple());
                                b.valid_from = at;
                                b.observed_at = at;
                                b.revision = 1;
                                store
                                    .collection(COLLECTION)
                                    .insert_one(document(&b).map_err(tx::domain)?)
                                    .session(&mut *session)
                                    .await
                                    .map_err(tx::mongo)?;
                                if let Some(parent) = evidence_parent {
                                    super::sandbox_usage_export::retain_binding_bridge(store, session, &b, parent, at).await?;
                                }
                            }
                            store
                                .collection(super::usage_reports::CONTROL)
                                .update_one(
                                    doc! {"_id":"observer"},
                                    doc! {"$inc":{"revision":1_i64}},
                                )
                                .session(&mut *session)
                                .await
                                .map_err(tx::mongo)?;
                        }
                    }
                    store
                        .fence_control_transaction("observer", holder, &mut *session)
                        .await?;
                    Ok(())
                })
            },
        )
        .await
    }
    pub(crate) async fn bindings(&self, ids: &[String], range: Range) -> Result<Vec<Binding>> {
        let mut session = self.snapshot().await?;
        self.bindings_at(ids, range, &mut session).await
    }
    pub(crate) async fn bindings_at(
        &self,
        ids: &[String],
        range: Range,
        session: &mut mongodb::ClientSession,
    ) -> Result<Vec<Binding>> {
        metric_bindings::read_bindings(
            self.collection(COLLECTION),
            ids,
            range,
            session,
            DB_DEADLINE,
        )
        .await
    }
    pub(crate) async fn tracking_started_at(
        &self,
        ids: &[String],
        session: &mut mongodb::ClientSession,
    ) -> Result<Option<u64>> {
        metric_bindings::read_tracking_start(self.collection(COLLECTION), ids, session, DB_DEADLINE)
            .await
    }
    pub(crate) async fn mapping_revision(&self) -> Result<u64> {
        let mut session = self.snapshot().await?;
        self.mapping_revision_at(&mut session).await
    }
    pub(crate) async fn mapping_revision_at(
        &self,
        session: &mut mongodb::ClientSession,
    ) -> Result<u64> {
        let doc = self
            .collection(super::usage_reports::CONTROL)
            .find_one(doc! {"_id":"observer"})
            .max_time(DB_DEADLINE)
            .session(&mut *session)
            .await
            .map_err(unavailable)?
            .ok_or(MetricsError::Unavailable)?;
        Ok(doc.get_i64("revision").map_err(unavailable)? as u64)
    }
    pub(crate) async fn expire_missing_bindings(&self, holder: &str, at: u64) -> Result<()> {
        // Close only at the last proven instant; polling delays are not proof
        // of ownership or of workload activity during the unobserved gap.
        let mut cursor = self
            .collection(COLLECTION)
            .find(
                doc! {"valid_until":null,"record.observed_at":{"$lt":at.saturating_sub(90) as i64}},
            )
            .limit(200)
            .max_time(DB_DEADLINE)
            .await
            .map_err(unavailable)?;
        while let Some(doc) = cursor.try_next().await.map_err(unavailable)? {
            let mut b: Binding = decode(&doc)?;
            let revision = b.revision;
            b.revision += 1;
            b.valid_until = Some(b.observed_at);
            tx::run(
                &self.client,
                (self, holder, &b, revision),
                |&(store, holder, b, revision), session| {
                    Box::pin(async move {
                        let changed = store
                            .collection(COLLECTION)
                            .replace_one(
                                doc! {"_id":&b.id,"revision":revision as i64},
                                document(b).map_err(tx::domain)?,
                            )
                            .session(&mut *session)
                            .await
                            .map_err(tx::mongo)?
                            .matched_count;
                        if changed == 1 {
                            store
                                .collection(super::usage_reports::CONTROL)
                                .update_one(
                                    doc! {"_id":"observer"},
                                    doc! {"$inc":{"revision":1_i64}},
                                )
                                .session(&mut *session)
                                .await
                                .map_err(tx::mongo)?;
                        }
                        store
                            .fence_control_transaction("observer", holder, &mut *session)
                            .await?;
                        Ok(())
                    })
                },
            )
            .await?;
        }
        Ok(())
    }
}
fn same_assignment(a: &Binding, b: &Binding) -> bool {
    a.deployment == b.deployment
        && a.cluster == b.cluster
        && a.namespace == b.namespace
        && a.native_sandbox_uid == b.native_sandbox_uid
        && a.claim_uid == b.claim_uid
        && a.sandbox_id == b.sandbox_id
        && a.owner_subject == b.owner_subject
        && a.pvc_uid == b.pvc_uid
        && a.pvc_name == b.pvc_name
        && a.kind == b.kind
        && (a.kind == BindingKind::Home
            || (a.pod_name == b.pod_name
                && a.pod_uid == b.pod_uid
                && a.runtime_container_id == b.runtime_container_id
                && a.restart_count == b.restart_count
                && a.runtime_generation == b.runtime_generation
                && a.cpu_limit_cores == b.cpu_limit_cores
                && a.memory_limit_bytes == b.memory_limit_bytes))
}
fn document(b: &Binding) -> Result<Document> {
    let mut d = doc! {"_id":&b.id,"document_schema_version":1_i64,"revision":b.revision as i64,"sandbox_id":&b.sandbox_id,"owner":&b.owner_subject,
    "physical_key":b.physical_key(),"kind":encode(&b.kind)?,"valid_from":b.valid_from as i64,"valid_until":b.valid_until.map(|v|v as i64),"record":encode(b)?};
    if let Some(until) = b.valid_until {
        d.insert("expires_at", expires(until + IDENTITY_RETENTION_SECONDS));
    } else {
        d.insert("active_physical", b.physical_key());
    }
    Ok(d)
}
