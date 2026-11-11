use super::*;
use crate::hosted::acp::rpc_result;

#[test]
fn projection_and_clones_keep_the_entire_root_until_final_drop() {
    let payload = br#"{"result":{"answer":42},"sibling":"held with root"}"#;
    let weight = tree_allowance(
        payload.len(),
        output_json_budget::inspect(payload).unwrap().nodes,
    )
    .unwrap();
    let pools = JsonPools::new(weight, SCRATCH_BYTES);
    let frame = pools.decode(payload).unwrap();
    assert_eq!(pools.available(), (0, SCRATCH_BYTES));
    let clone = frame.clone();
    let result = rpc_result("example", frame).unwrap();
    assert_eq!(result["answer"], 42);
    assert!(std::ptr::eq(&clone["result"], &*result));
    drop(clone);
    assert!(matches!(
        pools.decode(payload),
        Err(AdapterError::AtCapacity(_))
    ));
    assert_eq!(pools.available(), (0, SCRATCH_BYTES));
    let last = result.clone();
    drop(result);
    assert_eq!(pools.available().0, 0);
    drop(last);
    assert_eq!(pools.available(), (weight, SCRATCH_BYTES));
    assert!(pools.decode(payload).is_ok());
}

#[test]
fn result_error_and_missing_result_keep_legacy_semantics() {
    let pools = JsonPools::new(TREE_BYTES, SCRATCH_BYTES);
    for payload in [
        br#"{"result":23}"#.as_slice(),
        br#"{"missing":true}"#,
        br#"{"error":{"code":-1,"message":"refused"}}"#,
    ] {
        let legacy = JsonFrame::legacy(serde_json::from_slice(payload).unwrap());
        let admitted = pools.decode(payload).unwrap();
        let old = rpc_result("method", legacy);
        let new = rpc_result("method", admitted);
        match (old, new) {
            (Ok(old), Ok(new)) => assert_eq!(*old, *new),
            (Err(old), Err(new)) => assert_eq!(old.to_string(), new.to_string()),
            _ => panic!("RPC semantics changed"),
        }
        assert_eq!(pools.available(), (TREE_BYTES, SCRATCH_BYTES));
    }
}

#[test]
fn scratch_tree_shape_and_private_key_refusals_release_every_reservation() {
    let pools = JsonPools::new(TREE_BYTES, SCRATCH_BYTES);
    for payload in [
        b"{".to_vec(),
        format!("[{}]", vec!["0"; 8192].join(",")).into_bytes(),
        format!("{}0{}", "[".repeat(65), "]".repeat(65)).into_bytes(),
        vec![b' '; 4 * 1024 * 1024 + 1],
        br#"{"$serde_json::private::RawValue":"[0,0,0]"}"#.to_vec(),
        br#"{"\u0024serde_json::private::RawValue":"[]"}"#.to_vec(),
        br#"{"$serde_json::private::Number":"123"}"#.to_vec(),
    ] {
        assert!(matches!(
            pools.decode(&payload),
            Err(AdapterError::Rejected(_))
        ));
        assert_eq!(pools.available(), (TREE_BYTES, SCRATCH_BYTES));
    }
    for (tree, scratch) in [(0, SCRATCH_BYTES), (TREE_BYTES, 0)] {
        let pools = JsonPools::new(tree, scratch);
        assert!(matches!(
            pools.decode(b"{}"),
            Err(AdapterError::AtCapacity(_))
        ));
        assert_eq!(pools.available(), (tree, scratch));
    }
    assert!(scratch_allowance(usize::MAX).is_err());
    assert!(tree_allowance(1, usize::MAX).is_err());
}

#[tokio::test]
async fn reply_receiver_loss_and_cancelled_consumer_release_storage() {
    let pools = JsonPools::new(TREE_BYTES, SCRATCH_BYTES);
    let (send, recv) = tokio::sync::oneshot::channel();
    send.send(pools.decode(br#"{"result":1}"#).unwrap())
        .unwrap();
    assert!(pools.available().0 < TREE_BYTES);
    drop(recv);
    assert_eq!(pools.available(), (TREE_BYTES, SCRATCH_BYTES));
    let frame = pools.decode(br#"{"result":2}"#).unwrap();
    let task = tokio::spawn(async move {
        let _frame = frame;
        std::future::pending::<()>().await;
    });
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(pools.available(), (TREE_BYTES, SCRATCH_BYTES));
}

#[test]
fn process_pool_is_shared_and_duplicate_keys_follow_parser_semantics() {
    let a = JsonPools::process();
    let b = JsonPools::process();
    assert!(Arc::ptr_eq(&a.trees, &b.trees));
    assert!(Arc::ptr_eq(&a.scratch, &b.scratch));
    let pools = JsonPools::new(TREE_BYTES, SCRATCH_BYTES);
    let frame = pools.decode(br#"{"result":1,"result":2}"#).unwrap();
    assert_eq!(frame["result"], 2);
    drop(frame);
    assert_eq!(pools.available(), (TREE_BYTES, SCRATCH_BYTES));
}
