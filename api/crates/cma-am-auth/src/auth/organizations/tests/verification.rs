use futures_util::future::join_all;

use super::*;

impl Fixture {
    pub(super) async fn block_verification(&self) -> Arc<Semaphore> {
        let mut metadata = self.auth.metadata.write().await;
        let metadata = metadata.as_mut().unwrap();
        metadata.introspection_endpoint =
            Some(metadata.token_endpoint.replace("/token", "/introspect"));
        let gate = Arc::new(Semaphore::new(0));
        *self.control.verification_gate.write().await = Some(gate.clone());
        gate
    }
}

#[tokio::test]
async fn overlapping_organization_reads_share_verification_but_keep_memberships_fresh() {
    let fixture = Fixture::new().await;
    let gate = fixture.block_verification().await;
    let requests = join_all((0..8).map(|index| {
        let fixture = &fixture;
        async move {
            if index % 2 == 0 {
                let org = if index == 0 { "org-a" } else { "org-b" };
                let context = fixture.lookup(org).await.unwrap();
                assert_eq!(context.caller.subject, caller().subject);
                assert_eq!(context.memberships[0].organization.as_str(), org);
                assert_eq!(context.memberships[0].role, OrganizationRole::Admin);
                assert!(context.memberships[0].is_current(&context.credential, SystemTime::now()));
            } else {
                let choices = fixture
                    .auth
                    .organization_choices(&caller(), "fixture-credential")
                    .await
                    .unwrap();
                assert_eq!(choices.len(), 2);
            }
        }
    }));
    tokio::pin!(requests);
    // Poll every caller into the held provider verification before allowing it
    // to complete. The final counters detect duplicate work without sleeps.
    assert!(futures_util::poll!(&mut requests).is_pending());
    tokio::time::timeout(Duration::from_secs(2), async {
        tokio::select! {
            _ = &mut requests => panic!("verification must remain held"),
            permit = fixture.control.verification_arrivals.acquire() => permit.unwrap().forget(),
        }
    })
    .await
    .unwrap();
    gate.add_permits(8);
    tokio::time::timeout(Duration::from_secs(2), &mut requests)
        .await
        .unwrap();
    assert_eq!(fixture.control.userinfo_calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        fixture.control.introspection_calls.load(Ordering::SeqCst),
        1
    );
    // Choices never substitute for each caller's exact organization lookup.
    assert_eq!(fixture.control.lookups.load(Ordering::SeqCst), 4);
    assert_eq!(fixture.control.inventory_calls.load(Ordering::SeqCst), 4);

    *fixture.control.verification_gate.write().await = None;
    fixture.control.reply.write().await.1 = json!({"your_role":"viewer"});
    let next = fixture.lookup("org-a").await.unwrap();
    assert_eq!(next.memberships[0].role, OrganizationRole::Viewer);
    assert_eq!(fixture.control.userinfo_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        fixture.control.introspection_calls.load(Ordering::SeqCst),
        2
    );
    assert_eq!(fixture.control.lookups.load(Ordering::SeqCst), 5);

    *fixture.control.introspection.write().await = json!({"active":false});
    assert_eq!(
        fixture.lookup("org-a").await,
        Err(MembershipError::Unavailable)
    );
    assert_eq!(fixture.control.userinfo_calls.load(Ordering::SeqCst), 3);
    assert_eq!(
        fixture.control.introspection_calls.load(Ordering::SeqCst),
        3
    );
    assert_eq!(fixture.control.lookups.load(Ordering::SeqCst), 5);
}
