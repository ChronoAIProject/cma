//! Admitted read-only integrity proof composition.
use super::*;

/// Borrowed inputs retain their caller's custody and snapshot provenance. This
/// read-only proof never starts an audit, quarantine, commit, or abort.
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "enabled recording awaits driver resource custody")
)]
pub(in crate::store) fn read_only_summary_admitted<'a>(
    store: &'a MongoSandboxStore,
    agent: &'a Agent,
    raw: &'a mongodb::bson::RawDocument,
    session: &'a mut mongodb::ClientSession,
    snapshot: bool,
    policy: &'a IngressOwnershipPolicy,
    read_plan: AllocationPlan,
) -> Result<SummaryRead<'a>, AdmittedError> {
    AdmittedOperation::from_owned(
        policy,
        IngressClass::Transformation,
        AllocationPlan::checked(
            "integrity summary result",
            &[128, shared_owner_bytes::<ReadFailure>()],
        )?,
        move |scope| async move {
            let metadata = match row::decode::<proof::SummarySchema>(raw, policy) {
                Ok(value) => value,
                Err(error) => return error::finish(Err(ReadError::Row(error)), &scope),
            };
            let Some(integrity) = metadata.as_ref() else {
                if agent.conversation.last_seq == 0 {
                    let extra = match store
                        .integrity_point_read_admitted(
                            session,
                            snapshot,
                            true,
                            policy,
                            read_plan,
                            || doc! {"agent_id":agent.id.as_str()},
                        )?
                        .await?
                        .transpose()
                    {
                        Ok(value) => value,
                        Err(error) => return Ok(error.map(Err)),
                    };
                    if extra.is_some() {
                        return Ok(scope.initialize_owned(|| {
                            Err(ReadFailure::Domain(SandboxStoreError::Corrupt(
                                "legacy zero-head Agent has retained log rows".into(),
                            )))
                        }));
                    }
                }
                return Ok(scope.initialize_owned(|| Ok(None)));
            };
            let validation = IngressWorkScope::new(policy.admit(
                IngressClass::Transformation,
                proof::summary_validation_plan(agent)?,
            )?);
            let stale = match validation
                .initialize_owned(|| {
                    proof::validate_metadata(agent, integrity).map_err(ReadFailure::Domain)
                })
                .transpose()
            {
                Ok(value) => *value,
                Err(error) => return Ok(error.map(Err)),
            };
            drop(validation);
            if integrity.head_seq > 0 {
                if let Err(error) = store
                    .integrity_tail_admitted(
                        ExpectedTail {
                            agent_id: &agent.id,
                            sequence: integrity.head_seq,
                            hash: integrity.head_hash,
                        },
                        session,
                        snapshot,
                        policy,
                        read_plan,
                    )?
                    .await?
                    .transpose()
                {
                    return Ok(error.map(Err));
                }
            }
            if stale {
                return Ok(scope.initialize_owned(|| Ok(None)));
            }
            let extra = match store
                .integrity_point_read_admitted(session, snapshot, true, policy, read_plan, || {
                    let mut filter = doc! {"agent_id":agent.id.as_str()};
                    if integrity.head_seq > 0 {
                        filter.insert("seq", doc! {"$gt":integrity.head_seq as i64});
                    }
                    filter
                })?
                .await?
                .transpose()
            {
                Ok(value) => value,
                Err(error) => return Ok(error.map(Err)),
            };
            if extra.is_some() {
                return Ok(scope.initialize_owned(|| {
                    Err(ReadFailure::Domain(SandboxStoreError::Corrupt(
                        "Agent log has rows beyond its trusted head".into(),
                    )))
                }));
            }
            if !integrity.semantic.valid {
                return Ok(scope.initialize_owned(|| Ok(None)));
            }
            let summary = proof::copy_summary(&metadata, policy)?;
            Ok(scope.initialize_owned(|| Ok(Some(summary))))
        },
    )
}
