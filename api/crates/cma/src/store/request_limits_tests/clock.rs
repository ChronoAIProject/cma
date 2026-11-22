use super::super::*;
use super::{
    fixture::{replay, Fixture},
    legacy::LegacyLimits,
};
use mongodb::bson::{Bson, DateTime};

#[tokio::test]
#[ignore = "requires disposable MongoDB 8.0.13 replica set"]
async fn update_and_before_projection_share_now_even_across_delayed_evaluation() {
    let f = Fixture::new().await;
    f.left.check("template", 1).await.unwrap();
    let template = f.take().pop().unwrap();
    for index in 0..12 {
        let key = format!("clock-{index}");
        f.seed(&key, Bson::Int32(0)).await;
        // Apply the TTL deletion predicate before replay, independently of the monitor.
        let expired = f
            .left
            .limits
            .delete_one(doc! {"_id":&key, "$expr":{"$lte":["$expires_at", "$$NOW"]}})
            .await
            .unwrap();
        assert_eq!(
            expired.deleted_count, 0,
            "the clock test needs a retained preimage"
        );
        let mut command = replay(template.clone(), &key);
        // Retain the exact production update and preimage projection. Force time
        // to pass AFTER the update evaluates $$NOW but BEFORE projecting the
        // preimage. A projection that re-sampled wall time would fail equality.
        command.get_array_mut("update").unwrap().push(Bson::Document(doc!{"$set":{
            "expires_at":{"$function":{"body":"function(value) { const start = Date.now(); while (Date.now() - start < 75) {} return value; }","args":["$expires_at"],"lang":"js"}}
        }}));
        command
            .get_document_mut("fields")
            .unwrap()
            .insert("command_clock", doc! {"$toLong":"$$NOW"});
        let start = std::time::Instant::now();
        let reply = f.database.run_command(command).await.unwrap();
        assert!(start.elapsed() >= Duration::from_millis(75));
        let clock = reply
            .get_document("value")
            .unwrap()
            .get_i64("command_clock")
            .unwrap();
        let updated = f
            .left
            .limits
            .find_one(doc! {"_id":key})
            .await
            .unwrap()
            .unwrap();
        assert_eq!(updated.get_i64("tat").unwrap() - 60_000, clock);
        assert_eq!(
            updated
                .get_datetime("expires_at")
                .unwrap()
                .timestamp_millis()
                - 120_000,
            clock
        );
    }
    f.close().await;
}

pub(super) fn freeze(value: &mut Bson, now: DateTime) {
    match value {
        Bson::String(s) if s == "$$NOW" => *value = Bson::DateTime(now),
        Bson::Document(d) => {
            for (_, v) in d.iter_mut() {
                freeze(v, now);
            }
        }
        Bson::Array(a) => {
            for v in a {
                freeze(v, now);
            }
        }
        _ => {}
    }
}

#[tokio::test]
#[ignore = "requires disposable MongoDB 8.0.13 replica set"]
async fn retry_after_boundary_and_multi_token_match_legacy_server_expressions() {
    let f = Fixture::new().await;
    let old = LegacyLimits::open(&f.database).await.unwrap();
    // Replay commands captured from both real implementations. Replace only
    // their clock input with one literal so boundary +/-1 ms is deterministic;
    // all predicates, update expressions and rounding remain independently pinned.
    let now = DateTime::from_millis(DateTime::now().timestamp_millis() + 3_600_000);
    for (rate, tokens) in [(1, 1), (4, 1), (4, 3)] {
        f.seed(
            "template-old",
            Bson::Int64(now.timestamp_millis() + 120_000),
        )
        .await;
        f.take();
        assert!(old
            .charge("template-old", rate, tokens)
            .await
            .unwrap()
            .is_some());
        let old_commands = f.take();
        assert_eq!(old_commands.len(), 2);
        f.left.charge("template-new", rate, tokens).await.unwrap();
        let new_command = f.take().pop().unwrap();
        let interval = i64::from(60_000_u32.div_ceil(rate));
        let tolerance = interval * i64::from(rate - tokens);
        for delta in [-1_i64, 0, 1, 999, 1000, 1001] {
            let tat = now.timestamp_millis() + tolerance + delta;
            f.seed("old", Bson::Int64(tat)).await;
            f.seed("new", Bson::Int64(tat)).await;
            let mut old_update = Bson::Document(replay(old_commands[0].clone(), "old"));
            let mut new_update = Bson::Document(replay(new_command.clone(), "new"));
            freeze(&mut old_update, now);
            freeze(&mut new_update, now);
            let old_reply = f
                .database
                .run_command(old_update.as_document().unwrap().clone())
                .await
                .unwrap();
            let new_reply = f
                .database
                .run_command(new_update.as_document().unwrap().clone())
                .await
                .unwrap();
            let wait = new_reply
                .get_document("value")
                .unwrap()
                .get_i64("wait_ms")
                .unwrap();
            assert_eq!(wait, delta.max(0));
            assert_eq!(
                !matches!(old_reply.get("value"), Some(Bson::Null)),
                wait == 0
            );
            let mut aggregate = replay(old_commands[1].clone(), "old");
            aggregate.get_array_mut("pipeline").unwrap()[0] =
                Bson::Document(doc! {"$match":{"_id":"old"}});
            let mut aggregate = Bson::Document(aggregate);
            freeze(&mut aggregate, now);
            if delta > 0 {
                let reply = f
                    .database
                    .run_command(aggregate.as_document().unwrap().clone())
                    .await
                    .unwrap();
                let row = reply
                    .get_document("cursor")
                    .unwrap()
                    .get_array("firstBatch")
                    .unwrap()[0]
                    .as_document()
                    .unwrap();
                let old_wait = row
                    .get_i64("wait_ms")
                    .or_else(|_| row.get_i32("wait_ms").map(i64::from))
                    .unwrap();
                assert_eq!(
                    (wait as u64).div_ceil(1000),
                    (old_wait as u64).div_ceil(1000)
                );
                assert_eq!((wait as u64).div_ceil(1000), (delta as u64).div_ceil(1000));
            }
            let old_row = f
                .left
                .limits
                .find_one(doc! {"_id":"old"})
                .await
                .unwrap()
                .unwrap();
            let new_row = f
                .left
                .limits
                .find_one(doc! {"_id":"new"})
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                old_row.get_i64("tat").unwrap(),
                new_row.get_i64("tat").unwrap()
            );
        }
    }
    f.close().await;
}
