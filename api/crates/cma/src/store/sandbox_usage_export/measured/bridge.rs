use super::*;
use crate::store::{transaction_scope as tx, usage_reports::ReportingStore};
use layer2_core::{
    sandbox::Sandbox,
    sandbox_metrics::{Binding, BindingKind, MetricsError},
};

pub(crate) async fn retain_binding_bridge(
    store: &ReportingStore,
    session: &mut ClientSession,
    binding: &Binding,
    parent: &Sandbox,
    at: u64,
) -> tx::Result<()> {
    let resource = ResourceSnapshot::from_sandbox(parent);
    if binding.sandbox_id != parent.id.as_str()
        || binding.owner_subject != parent.owner.as_str()
        || resource.home_generation.as_deref() != Some(&binding.pvc_uid)
        || (binding.kind == BindingKind::Compute
            && resource.runtime_generation != binding.runtime_generation)
    {
        return Err(tx::domain(MetricsError::Unavailable));
    }
    let prefix = keys::bridge(&binding.id);
    let row = store
        .collection(INVENTORY)
        .find_one(keys::page(&prefix, None).map_err(|_| tx::domain(MetricsError::Unavailable))?)
        .sort(doc! {"_id":-1})
        .session(&mut *session)
        .await
        .map_err(tx::mongo)?;
    let mut proof = BindingProof {
        binding: binding.clone(),
        resource,
        verified_from: at,
        verified_until: at,
    };
    if let Some(row) = row {
        let old: BindingProof = decode(&row).map_err(|_| tx::domain(MetricsError::Unavailable))?;
        let same_resource = match binding.kind {
            BindingKind::Compute => old.resource == proof.resource,
            BindingKind::Home => {
                old.resource.home_id == proof.resource.home_id
                    && old.resource.home_generation == proof.resource.home_generation
            }
        };
        if !same_resource
            || old.binding.physical_key() != binding.physical_key()
            || old.binding.owner_subject != binding.owner_subject
            || at < old.verified_until
        {
            return Err(tx::domain(MetricsError::Unavailable));
        }
        if at - old.verified_until <= 90 {
            proof.verified_from = old.verified_from;
        }
    }
    let key = format!("{prefix}{:016x}", proof.verified_from);
    store
        .collection(INVENTORY)
        .replace_one(
            doc! {"_id":keys::id(&key)},
            document(&key, &proof).map_err(|_| tx::domain(MetricsError::Unavailable))?,
        )
        .upsert(true)
        .session(session)
        .await
        .map_err(tx::mongo)?;
    Ok(())
}
