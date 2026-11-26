use super::*;

fn download(replica: &Replica, id: &AgentId, method: Method) -> reqwest::RequestBuilder {
    client()
        .request(
            method,
            format!(
                "{}/api/v1/agents/{id}/files/download",
                replica.instance.address
            ),
        )
        .query(&[("path", FILE_PATH)])
}

pub(super) async fn revoke_admitted_read(
    owner: &Replica,
    peer: &Replica,
    id: &AgentId,
    authority: Arc<Authority>,
) -> Arc<Authority> {
    owner.runtime.pause_next.store(true, SeqCst);
    let request = download(peer, id, Method::GET).send();
    let revoke = async {
        tokio::time::timeout(Duration::from_secs(2), owner.runtime.entered.notified())
            .await
            .unwrap();
        authority.revoke();
    };
    let (response, ()) = tokio::join!(request, revoke);
    let response = response.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key(file_read::HANDOFF_HEADER));
    assert_eq!(response.bytes().await.unwrap().as_ref(), FILE_BYTES);
    assert_eq!(owner.runtime.reads.load(SeqCst), 4);
    assert_eq!(peer.runtime.reads.load(SeqCst), 1);
    assert_eq!(owner.probe.requests.load(SeqCst), 4);
    println!("file_handoff phase=admitted_read_revoked route=download status=200");

    owner
        .instance
        .coordinator
        .handoff(&authority, authority.revoke_transports())
        .await
        .unwrap();
    let replacement = owner
        .instance
        .coordinator
        .acquire(id)
        .await
        .unwrap()
        .unwrap();
    assert!(replacement.token.epoch > authority.token.epoch);
    replacement
}

pub(super) async fn close_outer_admission(
    owner: &Replica,
    peer: &Replica,
    id: &AgentId,
    authority: &Authority,
) {
    let old_read_count = owner.runtime.reads.load(SeqCst);
    let local_read_count = peer.runtime.reads.load(SeqCst);
    let old_entry_count = owner.probe.requests.load(SeqCst);
    let old_admission_refusals = owner.relay_log.refusals("admission_closed");
    owner.admission.close();
    assert_eq!(
        peer.instance.relay.destination(id).await.unwrap().token,
        authority.token
    );

    for forged_proof in [None, Some("unverified-synthetic-proof")] {
        let mut request = download(owner, id, Method::GET).header(file_read::HANDOFF_HEADER, "1");
        if let Some(proof) = forged_proof {
            request = request.header(PROOF, proof);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(response.headers()["retry-after"], "1");
        assert!(!response.headers().contains_key(file_read::HANDOFF_HEADER));
        let body: Value = response.json().await.unwrap();
        assert_eq!(
            body,
            serde_json::json!({"status": "not-ready", "reason": "draining"})
        );
    }
    assert_eq!(
        owner.relay_log.refusals("admission_closed"),
        old_admission_refusals
    );

    assert_reads(&reads(peer, id, "outer_admission_closed").await);
    let head = download(peer, id, Method::HEAD).send().await.unwrap();
    assert_eq!(head.status(), StatusCode::OK);
    assert!(!head.headers().contains_key(file_read::HANDOFF_HEADER));
    assert!(head.bytes().await.unwrap().is_empty());
    peer.relay_log
        .assert_outer_admission_correlated(&owner.relay_log);
    assert_eq!(
        owner.relay_log.refusals("admission_closed"),
        old_admission_refusals + 4,
    );
    assert_eq!(owner.runtime.reads.load(SeqCst), old_read_count);
    assert_eq!(owner.probe.requests.load(SeqCst), old_entry_count);
    assert_eq!(peer.runtime.reads.load(SeqCst), local_read_count + 4);
}
