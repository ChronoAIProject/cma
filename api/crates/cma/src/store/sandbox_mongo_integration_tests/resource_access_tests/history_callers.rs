use super::*;
use axum::http::StatusCode;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::SystemTime;

struct Membership {
    organization: SubjectId,
    calls: AtomicUsize,
    refusal: AtomicUsize,
}

#[async_trait::async_trait]
impl OrganizationAuthority for Membership {
    async fn organization_choices(
        &self,
        _: &Caller,
        _: &str,
    ) -> Result<Vec<OrganizationChoice>, MembershipError> {
        panic!("history must not enumerate organizations")
    }

    async fn organization_context(
        &self,
        caller: &Caller,
        _: &str,
        organization: &SubjectId,
    ) -> Result<CallerContext, MembershipError> {
        assert_eq!(organization, &self.organization);
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.refusal.load(Ordering::SeqCst) {
            1 => return Err(MembershipError::NotMember),
            2 => return Err(MembershipError::Unavailable),
            _ => {}
        }
        let mut context = fixture::context(caller.subject.clone());
        let now = SystemTime::now();
        context.memberships.push(OrganizationMembership::new(
            context.credential.clone(),
            organization.clone(),
            OrganizationRole::Viewer,
            now,
            now + MAX_MEMBERSHIP_AGE,
        )?);
        Ok(context)
    }
}

async fn audience(f: &Fixture, organization_owned: bool) -> Arc<Membership> {
    let snapshot = f
        .store
        .resource_access_snapshot(&ResourceTarget::Agent {
            agent_id: f.agent.id.clone(),
        })
        .await
        .unwrap();
    let organization = if organization_owned {
        snapshot.lineage.sandbox.owner.clone()
    } else {
        f.store
            .mutate_share(super::super::mutation(
                &snapshot,
                "history-reader",
                ShareMode::View,
            ))
            .await
            .unwrap();
        SubjectId::new("org")
    };
    Arc::new(Membership {
        organization,
        calls: AtomicUsize::new(0),
        refusal: AtomicUsize::new(0),
    })
}

async fn verify(organization_owned: bool) {
    let f = Fixture::with_owner_kind(260, organization_owned).await;
    let membership = audience(&f, organization_owned).await;
    let apps: Vec<_> = (0..4)
        .map(|mode| f.app_with_authority(mode % 2 == 1, mode >= 2, membership.clone()))
        .collect();
    let caller = fixture::context(SubjectId::new("history-reader"));
    for route in ["snapshot", "head_active", "head_idle"] {
        if route == "head_idle" {
            f.idle().await;
        }
        let endpoint = if route == "snapshot" {
            "snapshot"
        } else {
            "head"
        };
        let mut next = Some(format!("/api/v2/agents/{}/events/{endpoint}", f.agent.id));
        let mut page = 0;
        while let Some(path) = next {
            let mut expected = None;
            for (mode, app) in apps.iter().enumerate() {
                f.meter.clear();
                membership.calls.store(0, Ordering::SeqCst);
                let scan_before = scans();
                let (status, bytes) = f
                    .request_as(app, &path, caller.clone(), Some("fixture-credential"))
                    .await;
                assert_eq!(
                    status,
                    StatusCode::OK,
                    "{}",
                    String::from_utf8_lossy(&bytes)
                );
                let stats = f.meter.stats.lock().unwrap().clone();
                assert_eq!(membership.calls.load(Ordering::SeqCst), 1);
                // Non-owner entry keeps its pre/post-membership snapshots.
                // The private capture and final fence add one snapshot each.
                let baseline_snapshots = match route {
                    "snapshot" => 9,
                    "head_active" => 8,
                    _ => 5,
                };
                let snapshots = if mode >= 2 { 4 } else { baseline_snapshots };
                let per_snapshot = if mode % 2 == 1 { 2 } else { 3 };
                let baseline_log = match route {
                    "snapshot" if page == 0 => 6,
                    "snapshot" => 4,
                    "head_active" => 5,
                    _ => 0,
                };
                let commands = snapshots * per_snapshot + baseline_log
                    - u64::from(mode >= 2 && route != "head_idle");
                assert_eq!(stats.snapshots, snapshots);
                assert_eq!(stats.auth_commands, snapshots * per_snapshot);
                assert_eq!(stats.checkouts, commands);
                assert_eq!(stats.commands.values().sum::<u64>(), commands);
                if route == "snapshot" && mode >= 2 {
                    assert!(
                        commands * 100 <= (baseline_snapshots * per_snapshot + baseline_log) * 60
                    );
                }
                let scan_after = scans();
                assert_eq!(
                    scan_after[0] - scan_before[0],
                    u64::from(route != "head_idle")
                );
                assert_eq!(
                    scan_after[1] - scan_before[1],
                    if route == "head_idle" { 0 } else { 260 }
                );
                if let Some(expected) = &expected {
                    assert_eq!(&bytes, expected);
                } else {
                    expected = Some(bytes);
                }
            }
            let body: serde_json::Value = serde_json::from_slice(&expected.unwrap()).unwrap();
            next = body["next_page_cursor"].as_str().map(|cursor| {
                format!(
                    "/api/v2/agents/{}/events/snapshot?page_cursor={cursor}",
                    f.agent.id
                )
            });
            page += 1;
        }
        assert_eq!(page, if route == "snapshot" { 3 } else { 1 });
    }
    // Each fresh request must obtain membership again. Refusal must release
    // neither history nor private binding details and must not read the log.
    for refusal in [1, 2] {
        membership.refusal.store(refusal, Ordering::SeqCst);
        for endpoint in ["snapshot", "head"] {
            let path = format!("/api/v2/agents/{}/events/{endpoint}", f.agent.id);
            for (mode, app) in apps.iter().enumerate() {
                f.meter.clear();
                membership.calls.store(0, Ordering::SeqCst);
                let (status, bytes) = f
                    .request_as(app, &path, caller.clone(), Some("fixture-credential"))
                    .await;
                assert_eq!(
                    status,
                    if refusal == 1 {
                        StatusCode::NOT_FOUND
                    } else {
                        StatusCode::SERVICE_UNAVAILABLE
                    }
                );
                let stats = f.meter.stats.lock().unwrap().clone();
                assert_eq!(membership.calls.load(Ordering::SeqCst), 1);
                assert_eq!(stats.snapshots, 2);
                assert_eq!(stats.auth_commands, if mode % 2 == 1 { 4 } else { 6 });
                assert_eq!(stats.checkouts, stats.auth_commands);
                assert!(stats
                    .commands
                    .keys()
                    .all(|key| key.starts_with("authorization/")));
                let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
                assert!(body.get("items").is_none() && body.get("active_turn").is_none());
                for private in [
                    "Fixture request",
                    "Fixture steering",
                    "history-reader",
                    f.agent.id.as_str(),
                ] {
                    assert!(!String::from_utf8_lossy(&bytes).contains(private));
                }
            }
        }
    }
    f.close().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn organization_history_preserves_membership_and_round_trip_budgets() {
    crate::store::agent_log_telemetry::isolated_test(verify(true)).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn shared_history_preserves_membership_and_round_trip_budgets() {
    crate::store::agent_log_telemetry::isolated_test(verify(false)).await;
}
