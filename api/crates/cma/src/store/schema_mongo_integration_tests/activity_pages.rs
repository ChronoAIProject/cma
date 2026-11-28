use super::*;
use layer2_core::{
    ActivityAction, ActivityChannel, ActivityId, ActivityOutcome, ActivityQuery, ActivityRecord,
    ActivityResult, ActivityStore, ActivitySummary, ActivityTarget, DetailKey, Page, SubjectId,
    ACTIVITY_OUTCOME_UNCONFIRMED,
};

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn request_activity_survives_reopen_and_filters_before_paging() {
    let (client, name) = database().await;
    let retention = Duration::from_secs(90 * 86400);
    let first = MongoActivityStore::open(&client, &name, retention)
        .await
        .unwrap();
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let other_client = Client::with_uri_str(&uri).await.unwrap();

    let mut admitted = row(0);
    admitted.actor = None;
    admitted.outcome = ActivityOutcome::Failed(ACTIVITY_OUTCOME_UNCONFIRMED.into());
    admitted
        .detail
        .record(DetailKey::RequestId, "00000000-0000-4000-8000-000000000001");
    first.append(&admitted).await.unwrap();
    admitted.actor = Some(SubjectId::new("fixture"));
    admitted.detail.record(DetailKey::ActivityChannel, "web");
    first.identify_attempt(&admitted).await.unwrap();
    drop(first);

    let store = MongoActivityStore::open(&other_client, &name, retention)
        .await
        .unwrap();
    let web = ActivityQuery {
        channel: Some(ActivityChannel::Web),
        limit: 1,
        ..Default::default()
    };
    assert_eq!(
        store.list_summaries(&web).await.unwrap().items[0].outcome,
        admitted.outcome
    );
    store
        .finish_attempt(&admitted.id, &ActivityOutcome::Succeeded, 202)
        .await
        .unwrap();
    assert!(store
        .finish_attempt(&admitted.id, &ActivityOutcome::Refused("403".into()), 403)
        .await
        .is_err());
    store.append(&row(1)).await.unwrap();

    let matching = store
        .list_summaries(&ActivityQuery {
            actor: admitted.actor.clone(),
            channel: Some(ActivityChannel::Web),
            action: Some(ActivityAction::Update),
            result: Some(ActivityResult::Succeeded),
            recorded_from: Some("2020-01-01T00:00:00Z".into()),
            recorded_before: Some("2099-01-01T00:00:00Z".into()),
            limit: 1,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(matching.items.len(), 1);
    assert!(matching.next_cursor.is_none());
    let request = &matching.items[0];
    assert_eq!(request.id, admitted.id);
    assert_eq!(request.detail.get(DetailKey::HttpStatus), Some("202"));
    assert_eq!(
        request.detail.get(DetailKey::RequestId),
        admitted.detail.get(DetailKey::RequestId)
    );
    assert_eq!(request.detail.get(DetailKey::ResourceName), None);
    let legacy = store
        .list_summaries(&ActivityQuery {
            channel: Some(ActivityChannel::Unknown),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(legacy.items.len(), 1);
    assert_eq!(legacy.items[0].id, row(1).id);
    client.database(&name).drop().await.unwrap();
}

pub(super) fn row(index: usize) -> ActivityRecord {
    let mut row = ActivityRecord::new(
        if index.is_multiple_of(3) {
            ActivityAction::Update
        } else {
            ActivityAction::Read
        },
        ActivityOutcome::Succeeded,
    )
    .by(SubjectId::new(format!("fixture-{}", index % 7)))
    .on(ActivityTarget::Resource("/api/v1/sandboxes/{id}".into()));
    row.at = "2026-11-13T00:00:00Z".into();
    row.id = ActivityId(format!("act_{index:09}"));
    if index.is_multiple_of(3) {
        row.detail.record(DetailKey::ResourceName, "x".repeat(256));
        row.detail.record(DetailKey::Setting, "y".repeat(256));
    }
    row
}

pub(super) fn document(index: usize, at: DateTime) -> Document {
    let row = row(index);
    doc! {"_id":row.id.0.clone(),"document_schema_version":1_i64,"recorded_at":at,
    "record": mongodb::bson::serialize_to_bson(&row).unwrap()}
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn audit_filter_excludes_successful_reads_before_pagination() {
    let (client, name) = database().await;
    let store = MongoActivityStore::open(&client, &name, Duration::from_secs(90 * 86400))
        .await
        .unwrap();
    use ActivityAction::{Configure, GatewayRecordRead, List, Read};
    use ActivityOutcome::{Failed, Refused, Succeeded};
    let records = [
        row(0),
        ActivityRecord::new(Read, Succeeded),
        ActivityRecord::new(List, Succeeded),
        ActivityRecord::new(GatewayRecordRead, Succeeded),
        ActivityRecord::new(Read, Refused("forbidden".into())),
        ActivityRecord::new(GatewayRecordRead, Failed("source_unavailable".into())),
        ActivityRecord::new(Configure, Succeeded),
    ];
    let db = client.database(&name);
    let at = DateTime::now();
    db.collection::<Document>(ACTIVITY)
        .insert_many(records.iter().enumerate().map(|(offset, row)| {
            doc! {
                "_id": &row.id.0,
                "document_schema_version": 1_i64,
                "recorded_at": DateTime::from_millis(at.timestamp_millis() + offset as i64),
                "record": mongodb::bson::serialize_to_bson(row).unwrap(),
            }
        }))
        .await
        .unwrap();

    let mut query = ActivityQuery {
        exclude_successful_reads: true,
        limit: 2,
        ..Default::default()
    };
    let ids = |page: Page<ActivitySummary>| -> Vec<ActivityId> {
        page.items.into_iter().map(|item| item.id).collect()
    };
    let first = store.list_summaries(&query).await.unwrap();
    query.cursor = first.next_cursor.clone();
    assert!(query.cursor.is_some());
    assert_eq!(
        ids(first),
        vec![records[6].id.clone(), records[5].id.clone()]
    );
    let second = store.list_summaries(&query).await.unwrap();
    assert!(second.next_cursor.is_none());
    assert_eq!(
        ids(second),
        vec![records[4].id.clone(), records[0].id.clone()]
    );
    let read_query = ActivityQuery {
        action: Some(Read),
        ..Default::default()
    };
    let reads = store.list_summaries(&read_query).await.unwrap();
    assert_eq!(
        ids(reads),
        vec![records[4].id.clone(), records[1].id.clone()]
    );
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn activity_pages_keep_ties_filters_and_live_keyset_without_changing_audits() {
    let (client, name) = database().await;
    let store = MongoActivityStore::open(&client, &name, Duration::from_secs(90 * 86400))
        .await
        .unwrap();
    let db = client.database(&name);
    let at = DateTime::now();
    let collection = db.collection::<Document>(ACTIVITY);
    collection
        .insert_many((0..257).map(|i| document(i, at)))
        .await
        .unwrap();
    for (actor, action) in [
        (None, None),
        (Some(SubjectId::new("fixture-1")), None),
        (None, Some(ActivityAction::Read)),
        (
            Some(SubjectId::new("fixture-1")),
            Some(ActivityAction::Update),
        ),
    ] {
        let mut query = ActivityQuery {
            actor: actor.clone(),
            action,
            limit: 17,
            ..Default::default()
        };
        let mut actual = Vec::new();
        loop {
            let full = store.list(&query).await.unwrap();
            let page = store.list_summaries(&query).await.unwrap();
            assert_eq!(
                page.items,
                full.items
                    .into_iter()
                    .map(ActivitySummary::from)
                    .collect::<Vec<_>>()
            );
            assert_eq!(page.next_cursor, full.next_cursor);
            actual.extend(page.items.into_iter().map(|r| r.id.0));
            query.cursor = page.next_cursor;
            if query.cursor.is_none() {
                break;
            }
        }
        let expected: Vec<_> = (0..257)
            .rev()
            .map(row)
            .filter(|r| {
                actor.as_ref().is_none_or(|a| r.actor.as_ref() == Some(a))
                    && action.is_none_or(|a| r.action == a)
            })
            .map(|r| r.id.0)
            .collect();
        assert_eq!(actual, expected);
    }
    let first = store
        .list_summaries(&ActivityQuery {
            limit: 20,
            ..Default::default()
        })
        .await
        .unwrap();
    collection
        .insert_one(document(
            300,
            DateTime::from_millis(at.timestamp_millis() + 1),
        ))
        .await
        .unwrap();
    let next = store
        .list_summaries(&ActivityQuery {
            limit: 200,
            cursor: first.next_cursor,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(next.items[0].id.0, "act_000000236");
    assert!(!next.items.iter().any(|r| r.id.0 == "act_000000300"));
    let empty = store
        .list_summaries(&ActivityQuery {
            cursor: Some(format!("{}:act_000000000", at.timestamp_millis())),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(empty.items.is_empty() && empty.next_cursor.is_none());
    // Neither read path rewrites retained detail or installs the new index.
    assert_eq!(collection.count_documents(doc! {}).await.unwrap(), 258);
    assert_eq!(
        collection
            .find_one(doc! {"_id":"act_000000003"})
            .await
            .unwrap()
            .unwrap(),
        document(3, at)
    );
    let indexes = index_documents(&db, ACTIVITY).await;
    assert!(!indexes
        .iter()
        .any(|i| i.get_str("name").ok() == Some("activity_recorded_at_id_desc")));
    db.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires disposable replica-set MongoDB"]
async fn activity_projection_refuses_schema_and_required_field_corruption() {
    let (client, name) = database().await;
    let store = MongoActivityStore::open(&client, &name, Duration::from_secs(90 * 86400))
        .await
        .unwrap();
    let db = client.database(&name);
    let collection = db.collection::<Document>(ACTIVITY);
    for field in ["schema", "action", "outcome"] {
        let mut doc = document(1, DateTime::now());
        if field == "schema" {
            doc.insert("document_schema_version", 99_i64);
        } else {
            doc.get_document_mut("record")
                .unwrap()
                .insert(field, "invalid");
        }
        collection.insert_one(doc).await.unwrap();
        assert!(
            store
                .list_summaries(&ActivityQuery::default())
                .await
                .is_err(),
            "{field}"
        );
        collection.delete_many(doc! {}).await.unwrap();
    }
    db.drop().await.unwrap();
}

mod benchmark;
mod commands;
