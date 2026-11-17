//! Archive creation and displacement share captured ownership; contenders retain A.
use super::*;
#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set"]
async fn missing_authority_and_concurrent_preservation_cannot_erase_legacy_confirmation() {
    let mut h = Harness::new().await;
    let (owner, lease) = h.accepted().await;
    let old = legacy(&h, owner);
    let channel = h.current.channel.lock().unwrap().clone();
    let event = h.event();
    let (sent, ()) = tokio::join!(
        old.reply(caller(), &channel, &event, message(), false),
        async {
            let reply = crate::turns::routed::delivery_tests::command(&mut h.rx).await;
            let _permit = reply.gate.attempt().await.unwrap();
            reply.receipt.send(receipt()).unwrap();
        }
    );
    let original = sent.unwrap().unwrap();
    drop(old);
    let before = retained(&h, owner).await;
    assert!(matches!(
        ReplyStore(h.nodes[owner].journal(), None)
            .admit_keyed(&h.plan, h.target(), "b".repeat(64), "c".repeat(64))
            .await,
        Err(ExecutionError::LostLease)
    ));
    assert_eq!(retained(&h, owner).await, before);
    let store = ReplyStore(h.nodes[owner].journal(), lease.partition.clone());
    let (left, right) = tokio::join!(
        store.admit_keyed(&h.plan, h.target(), "b".repeat(64), "c".repeat(64)),
        store.admit_keyed(&h.plan, h.target(), "d".repeat(64), "e".repeat(64))
    );
    let mut claims = Vec::new();
    for result in [left, right] {
        match result {
            Ok(Admission::Claimed(claim)) => claims.push(claim),
            Ok(Admission::Pending) | Err(ExecutionError::Busy | ExecutionError::LostLease) => {}
            _ => panic!("unexpected competing preservation result"),
        }
    }
    assert_eq!(claims.len(), 1);
    let before = retained(&h, owner).await;
    assert!(before.0["legacy_confirmation"].is_string());
    let replay = h.tools[1 - owner]
        .reply(caller(), &channel, &event, message(), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        storage::bytes(&replay).unwrap(),
        storage::bytes(&original).unwrap()
    );
    store.finish(&claims[0], &receipt()).await.unwrap();
    store.close(&h.plan, h.target()).await.unwrap();
    let replay = h.tools[1 - owner]
        .reply(caller(), &channel, &event, message(), false)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        storage::bytes(&replay).unwrap(),
        storage::bytes(&original).unwrap()
    );
    assert!(h.rx.try_recv().is_err());
    h.cleanup().await;
}
