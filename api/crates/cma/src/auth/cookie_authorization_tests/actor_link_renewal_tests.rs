use super::*;

#[derive(Default)]
struct BatchStanding {
    calls: AtomicUsize,
    first_page: Notify,
}
#[async_trait::async_trait]
impl layer2_core::account_deletion::AccountStanding for BatchStanding {
    async fn erasing(
        &self,
        subject: &SubjectId,
    ) -> Result<bool, layer2_core::account_deletion::AccountDeletionError> {
        if self.calls.fetch_add(1, Ordering::SeqCst) + 1 == 100 {
            self.first_page.notify_one();
        }
        Ok(subject.as_str() == "deleting-owner")
    }
}

#[tokio::test]
async fn standing_gate_actor_link_renewal_reaches_other_owner_after_full_deleting_page() {
    let f = fixture().await;
    let (status, code) = candidate(&f).await;
    let grant = f
        .auth
        .confirm_actor_link(&actor(), status.revision, &SubjectId::new("owner"), &code)
        .await
        .unwrap();
    let mut template = f.auth.link_record(&grant.grant_id).await.unwrap();
    let revision = template.revision;
    template.renewed_at = 0;
    template.revision += 1;
    assert!(f
        .store
        .replace_actor_link(Some(revision), template.clone())
        .await
        .unwrap());
    let mut records = vec![template.clone()];
    for index in 0..100 {
        let mut sender = actor();
        sender.actor_id = format!("batch-sender-{index}");
        let status = f.auth.ensure_actor_link(&sender).await.unwrap();
        let initial = f.auth.link_record(&status.link_id).await.unwrap();
        let mut row = template.clone();
        row.actor = sender;
        row.actor_key = initial.actor_key;
        row.link_id = status.link_id;
        row.revision = initial.revision + 1;
        assert!(f
            .store
            .replace_actor_link(Some(initial.revision), row.clone())
            .await
            .unwrap());
        records.push(row);
    }
    // Select the healthy row outside the actual first page. This works with
    // either map iteration or the durable actor-key ordering used by pagination.
    let first_page = f.store.due_actor_links(u64::MAX, None, 100).await.unwrap();
    assert_eq!(first_page.len(), 100);
    let healthy = records
        .iter()
        .find(|row| !first_page.iter().any(|first| first.link_id == row.link_id))
        .unwrap()
        .clone();
    let mut deleting = Vec::new();
    for mut row in records
        .into_iter()
        .filter(|row| row.link_id != healthy.link_id)
    {
        let revision = row.revision;
        row.subject = Some(SubjectId::new("deleting-owner"));
        row.revision += 1;
        assert!(f
            .store
            .replace_actor_link(Some(revision), row.clone())
            .await
            .unwrap());
        deleting.push(row);
    }
    let standing = Arc::new(BatchStanding::default());
    let auth = AuthState::with_store(f.auth.fixture_mode().clone(), None, f.store.clone())
        .with_account_standing(standing.clone());
    *auth.fixture_metadata().write().await = f.auth.fixture_metadata().read().await.clone();
    auth.credential_for_grant(&healthy.link_id, &healthy.actor)
        .await
        .unwrap();
    standing.calls.store(0, Ordering::SeqCst);

    tokio::time::pause();
    let (shutdown, signal) = tokio::sync::watch::channel(false);
    let worker = auth.clone().spawn_actor_link_renewal(signal);
    let processed =
        tokio::time::timeout(Duration::from_secs(1), standing.first_page.notified()).await;
    let before_second_tick = auth.link_record(&healthy.link_id).await.unwrap();
    tokio::time::advance(Duration::from_secs(60)).await;
    // Resume real time for local provider IO after advancing the renewal tick.
    tokio::time::resume();
    let renewed = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let row = auth.link_record(&healthy.link_id).await.unwrap();
            if row.revision > healthy.revision && row.renewed_at > 0 {
                break row;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    shutdown.send(true).unwrap();
    worker.await.unwrap();
    processed.expect("the first tick must inspect its bounded page of 100 deleting owners");
    assert_eq!(before_second_tick.revision, healthy.revision);
    assert_eq!(before_second_tick.renewed_at, 0);
    let renewed = renewed.expect("the second tick must reach the healthy owner after the cursor");
    assert!(renewed.active && !renewed.revoked);
    assert_eq!(standing.calls.load(Ordering::SeqCst), 101);
    for expected in deleting {
        let retained = auth.link_record(&expected.link_id).await.unwrap();
        assert_eq!(retained.revision, expected.revision);
        assert_eq!(retained.renewed_at, 0);
        assert!(retained.active && !retained.revoked);
    }
}
