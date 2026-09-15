mod finish;
use super::*;
use crate::store::{durability::majority_collection, sandbox_mongo::record_bson};
use mongodb::{
    bson::{doc, Document},
    ClientSession, Collection, Database,
};

pub(super) async fn apply(
    store: &MongoBridgeAllocationStore,
    authority: &AllocationAuthority,
    lease: &BridgeControlPlaneLease,
    snapshot: &ChannelProvisioningSnapshot,
    change: &Change<'_>,
    session: &mut ClientSession,
) -> Result<ChannelStartupContext, MutationError> {
    let current = super::super::channel_provisioning::transaction::apply(
        store,
        authority,
        lease,
        &snapshot.allocation,
        &snapshot.state.settings,
        None,
        session,
    )
    .await?;
    if current.sandbox != snapshot.sandbox
        || current.creation != snapshot.creation
        || current.state != snapshot.state
    {
        return Err(AllocationError::Conflict.into());
    }
    let database = store.bridges.client.database(&store.records.namespace().db);
    let target = current.allocation.placement.target();
    let workspace = read(
        &database,
        "workspaces",
        target.workspace_id.as_str(),
        session,
    )
    .await?;
    let worktree = read(&database, "worktrees", target.worktree_id.as_str(), session).await?;
    let agent = read(&database, "agents", target.agent_id.as_str(), session).await?;
    let creation = read(
        &database,
        "agent_creation_provenance",
        target.agent_id.as_str(),
        session,
    )
    .await?;
    let context = ChannelStartupContext {
        workspace: super::super::channel_children::transaction::read_workspace(
            &current, &workspace,
        )?,
        worktree: crate::store::worktrees::read_worktree(&worktree)
            .map_err(|_| AllocationError::Corrupt)?,
        agent: crate::store::agents::read_agent(&agent).map_err(|_| AllocationError::Corrupt)?,
        creation: crate::store::agent_provenance::read(&creation)
            .map_err(|_| AllocationError::Corrupt)?,
        plan: agent
            .get("channel_startup")
            .map(|value| {
                mongodb::bson::deserialize_from_bson(value.clone())
                    .map_err(|_| AllocationError::Corrupt)
            })
            .transpose()?,
        snapshot: current,
    };
    context.require(authority)?;
    for (collection, document) in [
        ("workspaces", &workspace),
        ("worktrees", &worktree),
        ("agents", &agent),
        ("agent_creation_provenance", &creation),
    ] {
        let rows: Collection<Document> = majority_collection(&database, collection);
        let changed = rows
            .update_one(
                document.clone(),
                doc! {"$inc":{"channel_startup_fence":1_i64}},
            )
            .session(&mut *session)
            .await?;
        if changed.matched_count != 1 {
            return Err(AllocationError::Conflict.into());
        }
    }
    match change {
        Change::Read => Ok(context),
        Change::Prepare { previous, plan } => {
            require_same(&context, previous)?;
            prepare(&database, context, plan, session).await
        }
        Change::Finish {
            previous,
            plan,
            receipt,
            conversation,
            resources,
        } => {
            require_same(&context, previous)?;
            finish::apply(
                store,
                &database,
                context,
                plan,
                receipt,
                conversation,
                resources,
                session,
            )
            .await
        }
    }
}

async fn read(
    database: &Database,
    collection: &str,
    id: &str,
    session: &mut ClientSession,
) -> Result<Document, MutationError> {
    let rows: Collection<Document> = majority_collection(database, collection);
    rows.find_one(doc! {"_id":id})
        .session(&mut *session)
        .await?
        .ok_or(AllocationError::Corrupt.into())
}

fn require_same(
    current: &ChannelStartupContext,
    previous: &ChannelStartupContext,
) -> Result<(), AllocationError> {
    if current.workspace != previous.workspace
        || current.worktree != previous.worktree
        || current.agent != previous.agent
        || current.creation != previous.creation
        || current.plan != previous.plan
    {
        return Err(AllocationError::Conflict);
    }
    Ok(())
}

async fn prepare(
    database: &Database,
    mut context: ChannelStartupContext,
    plan: &ChannelStartupPlan,
    session: &mut ClientSession,
) -> Result<ChannelStartupContext, MutationError> {
    context.require_plan(plan)?;
    if context.plan.as_ref().is_some_and(|current| current != plan) {
        return Err(AllocationError::Conflict.into());
    }
    let fence = context
        .snapshot
        .allocation
        .lease
        .as_ref()
        .ok_or(AllocationError::LeaseUnavailable)?
        .fence;
    if context.agent.work.next_fence > fence {
        return Err(AllocationError::Conflict.into());
    }
    if context.plan.is_some() && context.agent.work.next_fence == fence {
        return Ok(context);
    }
    let previous_revision = context.agent.revision;
    context.agent.work.next_fence = fence;
    context.agent.work.resolved_skills = Some(plan.skills.clone());
    context.agent.work.cleanup_debt = plan.intents();
    context.agent.revision = context
        .agent
        .revision
        .checked_add(1)
        .ok_or(AllocationError::Corrupt)?;
    context.agent.updated_at = layer2_core::rfc3339_at(std::time::SystemTime::now());
    context.plan = Some(plan.clone());
    save_agent(database, &context, previous_revision, session).await?;
    Ok(context)
}

async fn save_agent(
    database: &Database,
    context: &ChannelStartupContext,
    previous_revision: u64,
    session: &mut ClientSession,
) -> Result<(), MutationError> {
    let mut fields = crate::store::agents::agent_document(&context.agent)
        .map_err(|_| AllocationError::Corrupt)?;
    fields.remove("_id");
    fields.insert(
        "channel_startup",
        record_bson(context.plan.as_ref().ok_or(AllocationError::Corrupt)?)
            .map_err(|_| AllocationError::Corrupt)?,
    );
    let rows: Collection<Document> = majority_collection(database, "agents");
    let changed = rows
        .update_one(
            doc! {"_id":context.agent.id.as_str(),"revision":previous_revision as i64},
            doc! {"$set":fields},
        )
        .session(&mut *session)
        .await?;
    if changed.matched_count != 1 {
        return Err(AllocationError::Conflict.into());
    }
    Ok(())
}
