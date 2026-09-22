use super::*;

async fn claim(c: &ChannelFixture, holder: &str) -> AdmittedAgentCommand {
    let now = SystemTime::now();
    c.f.resources
        .claim_next_command(CommandClaimRequest {
            agent_id: c.f.target.agent_id.clone(),
            holder: holder.into(),
            now: layer2_core::rfc3339_at(now),
            expires_at: layer2_core::rfc3339_at(now + Duration::from_secs(30)),
        })
        .await
        .unwrap()
        .unwrap()
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn idle_worker_handoff_avoids_duplicate_provider_check_and_preserves_fresh_writer() {
    let (c, moving) = moving_fixture(false).await;
    let record = held(&c).await;
    let (execution, sends) = execution(&c, false);
    let worker =
        AgentCommandWorker::new(c.f.resources.clone(), execution, "idle-handoff".into()).unwrap();
    worker
        .run_one(&record.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(sends.load(Ordering::SeqCst), 1);
    assert_eq!(
        moving.checks.load(Ordering::SeqCst),
        3,
        "admission, hold release, and fresh ordered writer remain authorized"
    );
    assert_eq!(
        moving.updates.load(Ordering::SeqCst),
        1,
        "writer must still refresh authority after transactional dispatch"
    );
    worker
        .run_one(&record.receipt.envelope.command_id)
        .await
        .unwrap();
    assert_eq!(
        sends.load(Ordering::SeqCst),
        1,
        "a completed dispatch cannot replay"
    );
    c.close().await;
}

#[tokio::test]
#[ignore = "requires the disposable MongoDB replica contract runner"]
async fn idle_handoff_reauthorizes_resource_drift_and_refuses_revocation_or_competing_claim() {
    for mutation in ["resource", "revocation", "claim"] {
        let (c, moving) = moving_fixture(false).await;
        let record = held(&c).await;
        let (execution, sends) = execution(&c, false);
        let handoff = execution
            .release_held_for_attempt(&record)
            .await
            .expect("fresh idle hold release is eligible for a one-use handoff");
        let claimed = claim(&c, "handoff-owner").await;
        let before =
            c.f.resources
                .agent_log(&c.f.target.agent_id, 0, 100)
                .await
                .unwrap();
        let mut expected = claimed.clone();
        match mutation {
            "resource" => {
                c.f.resources
                    .agents
                    .update_one(
                        doc! {"_id":c.f.target.agent_id.as_str()},
                        doc! {"$inc":{"revision":1_i64,"record.revision":1_i64}},
                    )
                    .await
                    .unwrap();
            }
            "revocation" => {
                c.f.service
                    .revoke(&caller(), None, c.f.revoke(&c.grant, "handoff-revoke"))
                    .await
                    .unwrap();
            }
            "claim" => {
                c.f.resources
                    .release_command_claim(
                        &claimed.work.lease.as_ref().unwrap().claim,
                        layer2_core::now_rfc3339(),
                    )
                    .await
                    .unwrap();
                expected = claim(&c, "competing-owner").await;
            }
            _ => unreachable!(),
        }
        let result = execution
            .execute_with_handoff(&claimed, Some(handoff))
            .await;
        if mutation == "resource" {
            assert_eq!(result, CommandAttempt::Finished);
            assert_eq!(sends.load(Ordering::SeqCst), 1);
            assert_eq!(
                moving.checks.load(Ordering::SeqCst),
                4,
                "resource drift requires full execution authorization before the fresh writer"
            );
        } else {
            assert_eq!(
                result,
                if mutation == "revocation" {
                    CommandAttempt::Reject(CommandFailureCode::AuthorityRevoked)
                } else {
                    CommandAttempt::Retry
                }
            );
            assert_eq!(sends.load(Ordering::SeqCst), 0, "{mutation}");
            assert_eq!(
                c.f.resources
                    .agent_log(&c.f.target.agent_id, 0, 100)
                    .await
                    .unwrap(),
                before
            );
            assert_eq!(
                c.f.resources
                    .get_command(&record.receipt.envelope.command_id)
                    .await
                    .unwrap(),
                Some(expected),
                "failed handoff cannot mutate a command or its current claim"
            );
        }
        c.close().await;
    }
}
