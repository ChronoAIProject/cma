use super::*;
use layer3_adapter::wire::RequestFrame;

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn reuse_lost_open_replays_one_child_conversation_and_preserves_parent_and_sibling() {
    for shared_workspace in [false, true] {
        let f = ReuseFixture::new(shared_workspace).await;
        let record = f.agent_start("sender-one").await;
        f.original
            .backend
            .lose_next_response(WireOperation::OpenAgent);
        assert!(f.step(record.clone()).await.is_err());
        let held = f
            .original
            .resources
            .get_agent(&record.placement.target().agent_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(held.status, AgentStatus::Starting);
        assert!(held.conversation.conversation_id.is_none());
        assert_eq!(held.work.cleanup_debt.len(), 3);
        let ready = f.drive_ready(record).await;
        let operation = format!("agent:{}:open:v1", ready.placement.target().agent_id);
        let opens = f
            .original
            .backend
            .requests()
            .await
            .into_iter()
            .filter_map(|(_, frame)| match frame {
                RequestFrame::OpenAgent(frame) if frame.request.operation_id == operation => {
                    Some(frame)
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(opens.len(), 2);
        assert_eq!(opens[0].request, opens[1].request);
        assert!(opens[1].authority_fence > opens[0].authority_fence);
        assert_eq!(
            f.original
                .resources
                .get_sandbox(&f.parent.id)
                .await
                .unwrap()
                .unwrap(),
            f.parent
        );
        assert_eq!(
            f.original
                .resources
                .get_agent(&f.sibling.id)
                .await
                .unwrap()
                .unwrap(),
            f.sibling
        );
        assert_eq!(f.original.server.state.mint_count.load(Ordering::SeqCst), 1);
        f.original.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn reuse_concurrent_startup_and_later_sender_reuse_keep_exact_child_identity() {
    for shared_workspace in [false, true] {
        let f = ReuseFixture::new(shared_workspace).await;
        let one = f.reserve("sender-one").await;
        let two = f.reserve("sender-two").await;
        let (one, two) = tokio::join!(f.drive_ready(one), f.drive_ready(two));
        let again = f.ready("sender-one").await;
        assert!(one == again);
        assert_ne!(
            one.placement.target().agent_id,
            two.placement.target().agent_id
        );
        let one = f
            .original
            .resources
            .get_worktree(&one.placement.target().worktree_id)
            .await
            .unwrap()
            .unwrap();
        let two = f
            .original
            .resources
            .get_worktree(&two.placement.target().worktree_id)
            .await
            .unwrap()
            .unwrap();
        assert_ne!(one.path, two.path);
        if shared_workspace {
            assert_ne!(one.name, two.name);
            assert_ne!(one.branch, two.branch);
        }
        assert_eq!(
            f.original
                .resources
                .get_sandbox(&f.parent.id)
                .await
                .unwrap()
                .unwrap(),
            f.parent
        );
        assert_eq!(
            f.original
                .resources
                .get_agent(&f.sibling.id)
                .await
                .unwrap()
                .unwrap(),
            f.sibling
        );
        f.original.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn reuse_reset_retains_old_children_and_reserved_capacity_without_moving_shared_parent() {
    for shared_workspace in [false, true] {
        let f = ReuseFixture::new(shared_workspace).await;
        let first = f.ready("sender-one").await;
        let agent = f
            .original
            .resources
            .get_agent(&first.placement.target().agent_id)
            .await
            .unwrap()
            .unwrap();
        let tree = f
            .original
            .resources
            .get_worktree(&first.placement.target().worktree_id)
            .await
            .unwrap()
            .unwrap();
        let next = f.reset(&first, "sender-one").await.unwrap();
        assert_eq!(next.identity.generation, first.identity.generation + 1);
        assert_eq!(
            next.placement.target().sandbox_id,
            first.placement.target().sandbox_id
        );
        assert_eq!(
            next.placement.target().workspace_id == first.placement.target().workspace_id,
            shared_workspace
        );
        assert_ne!(
            next.placement.target().agent_id,
            first.placement.target().agent_id
        );
        let next = f.drive_ready(next).await;
        let retired = f
            .original
            .allocations
            .generation(&first.identity)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retired.state, AllocationState::Retired);
        assert_eq!(retired.placement, first.placement);
        assert!(matches!(
            f.reset(&next, "sender-one").await,
            Err(AllocationError::Capacity)
        ));
        assert!(f.current(&next).await == next);
        assert_eq!(
            f.original
                .resources
                .get_agent(&agent.id)
                .await
                .unwrap()
                .unwrap(),
            agent
        );
        assert_eq!(
            f.original
                .resources
                .get_worktree(&tree.id)
                .await
                .unwrap()
                .unwrap(),
            tree
        );
        assert_eq!(
            f.original
                .resources
                .get_sandbox(&f.parent.id)
                .await
                .unwrap()
                .unwrap(),
            f.parent
        );
        assert_eq!(
            f.original
                .resources
                .get_agent(&f.sibling.id)
                .await
                .unwrap()
                .unwrap(),
            f.sibling
        );
        f.original.close().await;
    }
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn reuse_refuses_each_retained_channel_root_marker_and_reverse_admission_before_reservation()
{
    let f = ReuseFixture::new(true).await;
    let database = f.original.client.database(&f.original.database);
    let sandboxes = database.collection::<Document>("sandboxes");
    for marker in [
        "channel_creation",
        "channel_provisioning",
        "channel_renewal",
        "channel_lifecycle_credential",
        "channel_mint_reconciliation",
        "channel_retirement",
        "channel_cleanup",
    ] {
        sandboxes
            .update_one(doc! {"_id":f.parent.id.as_str()}, doc! {"$set":{marker:{}}})
            .await
            .unwrap();
        assert!(
            matches!(
                f.original
                    .resources
                    .require_ordinary_reuse_parent(&f.parent)
                    .await,
                Err(AllocationError::IncompatibleTarget)
            ),
            "{marker}"
        );
        let (admission, proof) = f.proposal("sender-one").await;
        assert!(
            matches!(
                f.original
                    .allocations
                    .reserve(AllocationReservation::new(admission, proof, now()).unwrap())
                    .await,
                Err(AllocationError::IncompatibleTarget)
            ),
            "{marker}"
        );
        sandboxes
            .update_one(
                doc! {"_id":f.parent.id.as_str()},
                doc! {"$unset":{marker:""}},
            )
            .await
            .unwrap();
    }
    let allocations = database.collection::<Document>("bridge_allocations");
    assert_eq!(allocations.count_documents(doc! {}).await.unwrap(), 0);
    allocations.insert_one(doc! {"_id":"fixture-root-admission", "sandbox_admission":{"sandbox_id":f.parent.id.as_str()}}).await.unwrap();
    assert!(matches!(
        f.original
            .resources
            .require_ordinary_reuse_parent(&f.parent)
            .await,
        Err(AllocationError::IncompatibleTarget)
    ));
    let (admission, proof) = f.proposal("sender-one").await;
    assert!(matches!(
        f.original
            .allocations
            .reserve(AllocationReservation::new(admission, proof, now()).unwrap())
            .await,
        Err(AllocationError::IncompatibleTarget)
    ));
    assert_eq!(allocations.count_documents(doc! {}).await.unwrap(), 1);
    assert_eq!(
        f.original
            .resources
            .get_sandbox(&f.parent.id)
            .await
            .unwrap()
            .unwrap(),
        f.parent
    );
    assert_eq!(
        f.original
            .resources
            .get_agent(&f.sibling.id)
            .await
            .unwrap()
            .unwrap(),
        f.sibling
    );
    f.original.close().await;
}
