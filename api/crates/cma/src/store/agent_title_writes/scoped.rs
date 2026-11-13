use super::*;
use crate::store::agent_log_segments::{
    publication::{self, PublicationIdentity},
    read_format,
    transaction_scope::TransactionScope,
    writer_commit::{self, AggregateUpdate, PublicationInput},
};

pub(super) async fn apply(
    store: &MongoSandboxStore,
    mutation: &TitleMutation,
    scope: &mut TransactionScope<'_>,
) -> Result<Option<TitleApply>, ApplyError> {
    let Some(row) = scope
        .find_one(&store.agents, doc! {"_id":mutation.id().as_str()})
        .await
        .map_err(ApplyError::Driver)?
    else {
        return Ok(None);
    };
    if read_format(&row).map_err(map_domain)?.is_none() {
        return Ok(None);
    }
    let current = read_agent(&row).map_err(map_domain)?;
    let state = read_title_state(&row, &current).map_err(ApplyError::Domain)?;
    if let Some(outcome) = mutation.settled(&state) {
        return Ok(Some(TitleApply::Settled(outcome)));
    }
    let (next, entry, stored) = next_title_state(&current, mutation)?;
    let mut fields = agent_replacement_fields(&next).map_err(map_domain)?;
    let completion = mutation.completion_fields();
    fields.extend(completion.clone());
    let mut filter = current_agent_filter(&current);
    filter.insert(
        TITLE,
        current
            .title
            .clone()
            .map(Bson::String)
            .unwrap_or(Bson::Null),
    );
    filter.insert(
        "record.title",
        current
            .title
            .clone()
            .map(Bson::String)
            .unwrap_or(Bson::Null),
    );
    if let TitleMutation::Automatic { claim, .. } = mutation {
        filter.extend(lease_filter(claim, state.stored.is_some()));
    }
    // Completion fields bind the exact original claim or user operation token.
    // They are allocated before retry and exclude the changing Agent head.
    let fingerprint =
        publication::request_digest(&(mutation.id(), &completion)).map_err(map_domain)?;
    let operation = PublicationIdentity {
        key: format!(
            "title:{}",
            layer2_core::profiles::content_digest(&completion)
                .map_err(|_| ApplyError::Domain(AgentTitleStoreError::Corrupt))?
        ),
        fingerprint,
        appended_at: mutation.set_at().into(),
    };
    let recorded = writer_commit::publish_existing(
        store,
        scope,
        PublicationInput {
            operation: &operation,
            current: &current,
            next: &next,
            logs: std::slice::from_ref(&entry),
            capacity: AppendCapacity::Normal,
            result_digest: publication::result_digest(&(&next, &entry, &completion))
                .map_err(map_domain)?,
        },
        AggregateUpdate { filter, fields },
    )
    .await
    .map_err(map_transaction_log)?;
    Ok(Some(TitleApply::Written(Box::new(TitleReceipt {
        segmented: Some(recorded),
        agent: next,
        entry,
        stored,
        proof: mutation.proof(),
        outcome: mutation.success_outcome(),
    }))))
}
