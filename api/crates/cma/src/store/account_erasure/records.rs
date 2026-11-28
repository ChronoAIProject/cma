//! One bounded, transaction-free record batch, suitable for run_resumable.
use super::{
    batches,
    catalog::{self, Erasure},
    context::{self, Context},
    guards, progress, selectors, *,
};

/// The caller must prove retired runtimes and final homes, retain the cohort
/// cursor, and durably union all discovered IDs and keys before acknowledging.
/// Billing mode is the caller's deployment authority, never inferred from rows.
pub(crate) async fn erase_records(
    db: &Database,
    targets: &[Target<'_>],
    cursor: &mut Cursor,
    now: &str,
    options: Options,
) -> Result<Batch, Error> {
    if targets.is_empty() {
        return Ok(Batch::default());
    }
    catalog::validate_catalog()
        .map_err(|name| Error::Store(format!("invalid erasure catalog: {name}")))?;
    if mongodb::bson::DateTime::parse_rfc3339_str(now).is_err() {
        return Err(Error::Store("erasure worker clock is invalid".into()));
    }
    let cohort: BTreeSet<_> = targets
        .iter()
        .map(|target| target.subject.as_str().to_owned())
        .collect();
    if cohort.len() != targets.len()
        || cohort.iter().any(|subject| subject.is_empty())
        || cohort.len() > 64
    {
        return Err(Error::Store("erasure cohort is invalid".into()));
    }
    super::barrier::validate(options)?;
    let operations = progress::operations();
    progress::prepare(cursor, cohort, options, &operations);
    cursor.capacity_subject = None;
    if targets.is_empty() {
        return Ok(Batch::default());
    }
    let mut contexts = std::mem::take(&mut cursor.contexts);
    let mut batch = Batch::default();
    let input = RunInput {
        db,
        targets,
        now,
        options,
    };
    let result = run(input, cursor, &mut contexts, &mut batch).await;
    cursor.contexts = contexts;
    // Error paths must obey the same persistence limit as successful pages.
    // Keep the original attributed cause while releasing an oversized cache.
    let outgoing = progress::cursor_capacity(cursor);
    let result = result.and(outgoing);
    progress::bind(cursor, &operations);
    let result = result.map_err(|error| match error {
        // Environmental sites return Store explicitly. Deterministic damage
        // without ownership evidence blocks the cohort for manual repair.
        Error::InvalidRecord(collection) => {
            Error::cohort(CohortBlock::CorruptRecord { collection })
        }
        other => other,
    });
    match result {
        Ok(()) => Ok(batch),
        Err(error)
            if !batch.deleted.is_empty()
                || !batch.redacted.is_empty()
                || !batch.pending_key_ids.is_empty()
                || !batch.discovered.is_empty() =>
        {
            batch.has_more = true;
            batch.waiting |= matches!(error, Error::Blocked(_) | Error::CohortBlocked(_));
            Err(Error::Interrupted {
                error: Box::new(error),
                batch: Box::new(batch),
            })
        }
        Err(error) => Err(error),
    }
}
struct RunInput<'a, 'b> {
    db: &'a Database,
    targets: &'a [Target<'b>],
    now: &'a str,
    options: Options,
}
async fn run(
    input: RunInput<'_, '_>,
    cursor: &mut Cursor,
    contexts: &mut Vec<Context>,
    batch: &mut Batch,
) -> Result<(), Error> {
    let RunInput {
        db,
        targets,
        now,
        options,
    } = input;
    let receipt = super::optional::inspect(
        db,
        options.allocation_rollups_enabled,
        options.agent_shared_observation,
    )
    .await?;
    if cursor.allocation_absent != Some(receipt.allocation_absent)
        || cursor.observer_absent != Some(receipt.observer_absent)
        || cursor.usage_projections_absent != Some(receipt.usage_projections_absent)
        || cursor.execution_namespaces != receipt.execution_namespaces
    {
        // A persisted absence marker cannot hide a namespace created later.
        cursor.operation = 0;
        cursor.after = None;
        cursor.selector = None;
        cursor.indexes_ready = false;
        cursor.allocation_absent = Some(receipt.allocation_absent);
        cursor.observer_absent = Some(receipt.observer_absent);
        cursor.usage_projections_absent = Some(receipt.usage_projections_absent);
        cursor
            .execution_namespaces
            .clone_from(&receipt.execution_namespaces);
    }
    if !cursor.indexes_ready {
        super::indexes::ensure_with_receipt(db, &receipt).await?;
        cursor.indexes_ready = true;
    }
    if !super::barrier::ready(db, targets, cursor, options, batch).await? {
        return Ok(());
    }
    if contexts.len() != targets.len() {
        *contexts = context::load_cohort_with_receipt(db, targets, &receipt).await?;
    }
    let mut expanded = if receipt.usage_projections_absent {
        false
    } else {
        context::discover_usage_projections(db, contexts).await?
    };
    for context in contexts.iter_mut() {
        let target = targets
            .iter()
            .find(|target| target.subject.as_str() == context.subject)
            .ok_or(Error::InvalidRecord("erasure cohort"))?;
        expanded |= context
            .merge_inventory(db, target.inventory)
            .await
            .map_err(|error| error.attribute(&context.subject))?;
        context
            .refresh(db)
            .await
            .map_err(|error| error.attribute(&context.subject))?;
    }
    if expanded {
        cursor.operation = 0;
        cursor.after = None;
        cursor.selector = None;
        cursor.billing_after = None;
        cursor.billing_checked = false;
    }
    progress::capacity(contexts)?;
    progress::cached_capacity(cursor, contexts)?;
    guards::preflight(
        db,
        targets,
        contexts,
        now,
        options,
        cursor.billing_checked,
        batch,
    )
    .await?;
    progress::handoff(contexts, targets, batch);
    if batch.waiting {
        return Ok(());
    }
    if !cursor.billing_checked {
        guards::billing_page(db, contexts, cursor).await?;
        batch.has_more = true;
        return Ok(());
    }
    let subjects = contexts
        .iter()
        .map(|context| context.subject.clone())
        .collect::<BTreeSet<_>>();
    let agents = selectors::agents(contexts);
    if cursor.retained_profiles.is_none() {
        super::profile_refs::page(db, contexts, cursor).await?;
        progress::cached_capacity(cursor, contexts)?;
        batch.has_more = true;
        return Ok(());
    }
    let operations = progress::operations();
    while let Some((entry, indexed)) = operations.get(cursor.operation).copied() {
        if receipt.skips(entry.collection) {
            cursor.advance();
            continue;
        }
        let rules: Vec<_> = entry
            .rules
            .iter()
            .filter(|rule| rule.indexed == indexed)
            .map(|rule| rule.erasure)
            .collect();
        let page = if indexed {
            super::indexed::page(
                db,
                entry,
                contexts,
                cursor.selector.as_ref(),
                cursor.after.as_ref(),
            )
            .await?
        } else {
            let rows = super::scan::page(db, entry.collection, cursor.after.as_ref()).await?;
            let after = rows.last().and_then(|row| row.get("_id")).cloned();
            super::indexed::Page {
                complete: rows.len() < 500,
                rows,
                selector: None,
                after,
            }
        };
        let rows = &page.rows;
        if rows.is_empty() {
            if page.complete {
                cursor.advance();
                continue;
            }
            cursor.selector = page.selector;
            cursor.after = page.after;
            batch.has_more = true;
            return Ok(());
        }
        let mut ids = vec![];
        for row in rows {
            for rule in &rules {
                if matches!(
                    rule,
                    Erasure::ProvenAbsent(_) | Erasure::ProvenAbsentJson(_)
                ) {
                    if let Some(context) = contexts
                        .iter()
                        .find(|context| batches::matches(entry, *rule, row, context))
                    {
                        return Err(Error::blocked(
                            &context.subject,
                            ErasureBlock::UnsupportedResidue {
                                collection: entry.collection.into(),
                            },
                        ));
                    }
                }
            }
            let destructive = rules.iter().any(|rule| {
                selectors::destructive(*rule)
                    && contexts
                        .iter()
                        .any(|context| batches::matches(entry, *rule, row, context))
            });
            if destructive {
                if super::resource_execution::managed(entry.collection) {
                    if !options
                        .drain_qualification
                        .is_some_and(TransactionDrainQualification::allows_execution_erasure)
                    {
                        return Err(Error::cohort(
                            CohortBlock::ExecutionErasureQualificationRequired,
                        ));
                    }
                    super::resource_execution::erase(db, entry.collection, row, contexts, batch)
                        .await?;
                    continue;
                }
                if entry.collection == "sandbox_billing_state" && guards::unsettled_billing(row) {
                    let context = contexts
                        .iter()
                        .find(|context| {
                            rules
                                .iter()
                                .any(|rule| batches::matches(entry, *rule, row, context))
                        })
                        .ok_or(Error::InvalidRecord("billing subject"))?;
                    return Err(Error::blocked(
                        &context.subject,
                        ErasureBlock::BillingSettlementPending,
                    ));
                }
                ids.push(
                    row.get("_id")
                        .cloned()
                        .ok_or(Error::InvalidRecord(entry.collection))?,
                );
            } else if rules.iter().any(|rule| {
                matches!(rule, Erasure::Reference(_))
                    && contexts
                        .iter()
                        .any(|context| selectors::matches(*rule, row, context))
            }) {
                let restart_drain = batches::redact(
                    db,
                    entry,
                    row,
                    batches::IdentityScope {
                        subjects: &subjects,
                        agents: &agents,
                    },
                    now,
                    cursor.retained_profiles.as_mut().unwrap(),
                    batch,
                )
                .await
                .map_err(|error| match responsible(row, contexts) {
                    Some(subject) => error.attribute(subject),
                    None => error,
                })?;
                if restart_drain {
                    // A pre-gate insert can finish after the first drain. Fence
                    // that new identity before refreshing any reference graph.
                    cursor.drain_complete = false;
                    cursor.drain_until = None;
                    cursor.retained_profiles = None;
                    cursor.retained_profile_owners.clear();
                    cursor.profile_discovery = None;
                    cursor.operation = 0;
                    cursor.after = None;
                    cursor.selector = None;
                    cursor.billing_checked = false;
                    cursor.billing_after = None;
                    contexts.clear();
                    return Ok(());
                }
            }
        }
        batches::remove(
            db,
            entry,
            batches::DeletePage { rows, ids: &ids },
            contexts,
            now,
            options,
            batch,
        )
        .await?;
        let conflict = batch.has_more || batch.waiting;
        if !conflict {
            cursor.selector = page.selector;
            cursor.after = page.after;
            if page.complete {
                cursor.advance();
            }
        }
        batch.has_more = cursor.operation < operations.len();
        return Ok(());
    }
    Ok(())
}

fn responsible<'a>(row: &mongodb::bson::Document, contexts: &'a [Context]) -> Option<&'a str> {
    fn contains(value: &mongodb::bson::Bson, subject: &str) -> bool {
        match value {
            mongodb::bson::Bson::String(value) => value == subject,
            mongodb::bson::Bson::Document(value) => {
                value.values().any(|value| contains(value, subject))
            }
            mongodb::bson::Bson::Array(values) => {
                values.iter().any(|value| contains(value, subject))
            }
            _ => false,
        }
    }
    contexts
        .iter()
        .find(|context| {
            row.values().any(|value| contains(value, &context.subject))
                || ["_id", "profile_id"].into_iter().any(|field| {
                    row.get_str(field)
                        .is_ok_and(|id| context.sets.values().any(|ids| ids.contains(id)))
                })
        })
        .map(|context| context.subject.as_str())
}
