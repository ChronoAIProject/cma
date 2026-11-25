use super::*;
use crate::replicas::mongo_fixture::Fixture;
use cmaeg_core::{channel::ProfileRevisions, seal::seal_blob};

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo"]
async fn first_assignment_commits_only_with_the_accepted_event_and_channel_version() {
    let fixture = Fixture::new().await;
    let left = fixture.records("https://fixture.example").await;
    let right = fixture.records("https://fixture.example").await;
    let store = Store::new(left.clone(), fixture.keys()).unwrap();
    let peer = Store::new(right.clone(), fixture.keys()).unwrap();
    let request = crate::ingress::routing_plan::fixture::request(
        |value| value["record_id"] = uuid::Uuid::new_v4().to_string().into(),
        crate::ingress::routing_plan::fixture::message("assignment"),
    );
    let mut channel = (*request.channel).clone();
    channel.definition.secrets.profile_revisions = Some(ProfileRevisions {
        current_revision: 1,
        baseline_complete: true,
        last_publication: None,
        retained: Vec::new(),
    });
    channel.definition.validate().unwrap();
    let blob = seal_blob(
        &fixture.keys(),
        &serde_json::to_vec(&channel.definition.to_blob_json()).unwrap(),
    )
    .unwrap();
    channel.version = left
        .put(
            &channel.definition.record_id,
            &channel.definition.owner.subject,
            &blob,
            None,
        )
        .await
        .unwrap();
    let consumer = ConsumerId::default_consumer();
    let conversation = &request.conversation_id;
    let (candidate, write) = prepare(&store, &channel, &consumer, conversation, true)
        .await
        .unwrap()
        .unwrap();
    let id = candidate.id.clone();
    assert!(peer.read::<Assignment>(&id, false).await.unwrap().is_none());
    let stale = Admission {
        channel_record_id: channel.definition.record_id.to_string(),
        channel_version: channel.version + 1,
        insert: write,
    };
    let expiry = time::OffsetDateTime::now_utc().unix_timestamp() + 600;
    assert!(left
        .put_profile_admission(
            &uuid::Uuid::new_v4().to_string(),
            &channel.definition.owner.subject,
            b"fixture",
            expiry,
            &stale,
            &[],
            None
        )
        .await
        .is_err());
    assert!(peer.read::<Assignment>(&id, false).await.unwrap().is_none());
    let (_, first) = prepare(&store, &channel, &consumer, conversation, true)
        .await
        .unwrap()
        .unwrap();
    let (_, second) = prepare(&peer, &channel, &consumer, conversation, true)
        .await
        .unwrap()
        .unwrap();
    let admission = |insert| Admission {
        channel_record_id: channel.definition.record_id.to_string(),
        channel_version: channel.version,
        insert,
    };
    let a = admission(first);
    let b = admission(second);
    let owner = &channel.definition.owner.subject;
    let first_id = uuid::Uuid::new_v4().to_string();
    let second_id = uuid::Uuid::new_v4().to_string();
    let (one, two) = tokio::join!(
        left.put_profile_admission(&first_id, owner, b"fixture", expiry, &a, &[], None),
        right.put_profile_admission(&second_id, owner, b"fixture", expiry, &b, &[], None)
    );
    assert_eq!(usize::from(one.is_ok()) + usize::from(two.is_ok()), 1);
    let (retained, version) = peer.read::<Assignment>(&id, false).await.unwrap().unwrap();
    assert_eq!(retained, candidate);
    assert_eq!(version, 1);
    let (again, insert) = prepare(&peer, &channel, &consumer, conversation, true)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again, retained);
    assert_eq!(insert.as_ref().and_then(|guard| guard.expected), Some(1));
    let stale_admission = admission(insert);
    peer.write(
        &id,
        owner,
        &retained,
        Write {
            expected: Some(1),
            channel: Some((channel.definition.record_id.to_string(), channel.version)),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(right
        .put_profile_admission(
            &uuid::Uuid::new_v4().to_string(),
            owner,
            b"fixture",
            expiry,
            &stale_admission,
            &[],
            None
        )
        .await
        .is_err());
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo"]
async fn profile_baseline_fences_legacy_roots_and_pins_prebaseline_admissions() {
    use crate::ingress::routing_plan::{plan, OrdinaryPlan, RoutingOptions};
    use crate::ingress::{
        routing_plan::fixture as ingress,
        routing_store::{Candidate, PlanStore},
    };
    use cmaeg_core::{
        channel::{ProfilePublication, RetainedProfileRevision, Secret},
        ids::{BindingId, EventId},
    };
    use std::{collections::BTreeSet, sync::Arc};

    let fixture = Fixture::new().await;
    let left = fixture.records("https://fixture.example").await;
    let right = fixture.records("https://fixture.example").await;
    let keys = Arc::new(ingress::keys());
    let store = Store::new(right.clone(), keys.clone()).unwrap();
    let now = time::OffsetDateTime::now_utc().unix_timestamp();
    let left_plans = PlanStore::new(left.clone(), keys.clone()).with_clock(Arc::new(move || now));
    let right_plans = PlanStore::new(right.clone(), keys.clone()).with_clock(Arc::new(move || now));
    let record_id = uuid::Uuid::new_v4().to_string();
    let mut request = ingress::request(
        |value| value["record_id"] = record_id.clone().into(),
        ingress::message("old admission"),
    );
    ingress::rewrite_ref(&mut request, |event| event.expires_at = now + 900);
    let mut current = (*request.channel).clone();
    let worker_hash =
        super::super::resolver::decode_key_hash(&current.definition.secrets.agent_key_hash)
            .unwrap();
    current.agent_key_hash = worker_hash;
    current
        .consumer_keys
        .push((ConsumerId::default_consumer(), worker_hash));
    assert!(requires_guard(&current.definition, true));
    assert!(current.definition.secrets.profile_revisions.is_none());
    let old_binding = current.definition.provider.clone();
    let owner = current.definition.owner.subject.clone();
    let persist = |definition: &ResolvedChannel| {
        seal_blob(
            &keys,
            &serde_json::to_vec(&definition.definition.to_blob_json()).unwrap(),
        )
        .unwrap()
    };
    current.version = left
        .put(
            &current.definition.record_id,
            &owner,
            &persist(&current),
            None,
        )
        .await
        .unwrap();
    request.channel = Arc::new(current.clone());
    let mut before = Candidate::prepare_compatibility(&request, b"old", &keys, now).unwrap();
    before.profile_admission = Some(channel_guard(&current));
    let old = left_plans.admit(&before).await.unwrap();
    assert_eq!(old.targets[0].launch.provider, old_binding);
    let mut next_activity = ingress::message("later admission");
    next_activity.event_id = EventId::new("event-next").unwrap();
    let mut delayed = ingress::request(
        |value| value["record_id"] = record_id.clone().into(),
        next_activity,
    );
    ingress::rewrite_ref(&mut delayed, |event| event.expires_at = now + 900);
    delayed.channel = Arc::new(current.clone());
    let OrdinaryPlan::Routed(plan) = plan(
        &delayed,
        &keys,
        now,
        RoutingOptions {
            enabled: true,
            attributes_signed: true,
            non_human_providers: &BTreeSet::from(["cma_codex".into()]),
        },
    )
    .unwrap() else {
        panic!("expected routed admission");
    };
    let mut stale = Candidate::prepare(&delayed, &plan, b"later", &keys, now).unwrap();
    stale.profile_admission = Some(channel_guard(&current));

    // Baseline starts on the other replica. The original root has no provider
    // mapping yet, so the retained admission supplies its original partition.
    current.definition.secrets.profile_revisions = Some(ProfileRevisions {
        current_revision: 1,
        baseline_complete: false,
        last_publication: None,
        retained: Vec::new(),
    });
    current.version = right
        .put(
            &current.definition.record_id,
            &owner,
            &persist(&current),
            Some(current.version),
        )
        .await
        .unwrap();
    let page = right_plans.recover_page(&owner, None, 32).await.unwrap();
    assert!(page.issues.is_empty());
    let retained = page
        .plans
        .iter()
        .find(|plan| plan.identity == old.identity)
        .unwrap();
    baseline(
        &store,
        &current,
        &retained.targets[0].launch.event.conversation_id,
    )
    .await
    .unwrap();

    let old_credential = current
        .definition
        .secrets
        .provider_credential
        .clone()
        .unwrap();
    let history = current
        .definition
        .secrets
        .profile_revisions
        .as_mut()
        .unwrap();
    history.retained.push(RetainedProfileRevision {
        revision: 1,
        profile_digest: current.definition.profile_digest.clone(),
        profile: serde_json::to_value(&current.definition.profile).unwrap(),
        provider: old_binding.clone(),
        credential: old_credential,
    });
    history.baseline_complete = true;
    history.current_revision = 2;
    history.last_publication = Some(ProfilePublication {
        expected_version: current.version,
        request_fingerprint: hash_secret("fixture-publication"),
    });
    current.definition.profile.context.system_prompt = "new profile".into();
    current.definition.profile_digest = current.definition.profile.digest();
    current.definition.provider.binding_id = BindingId::new("bnd_new_profile_fixture").unwrap();
    current.definition.secrets.provider_credential = Some(Secret::new("new-fixture-worker"));
    current.definition.secrets.agent_key_hash = hash_secret("new-fixture-worker");
    current.definition.validate().unwrap();
    current.version = right
        .put(
            &current.definition.record_id,
            &owner,
            &persist(&current),
            Some(current.version),
        )
        .await
        .unwrap();
    assert!(left_plans.admit(&stale).await.is_err());

    // Re-enter selection with the live version. Both the old committed root
    // and the new admission retain the original binding for this partition.
    let (assignment, guard) = prepare(
        &store,
        &current,
        &ConsumerId::default_consumer(),
        &delayed.conversation_id,
        true,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(assignment.revision, 1);
    delayed.channel = project(&current, &assignment).unwrap();
    let mut fresh = Candidate::prepare_compatibility(&delayed, b"later", &keys, now).unwrap();
    fresh.profile_admission = Some(Admission {
        channel_record_id: current.definition.record_id.to_string(),
        channel_version: current.version,
        insert: guard,
    });
    let accepted = right_plans.admit(&fresh).await.unwrap();
    assert_eq!(accepted.targets[0].launch.provider, old_binding);
    fixture.cleanup().await;
}
