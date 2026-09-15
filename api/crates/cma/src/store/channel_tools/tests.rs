use super::*;

impl MongoChannelToolStore {
    /// Read the actual encrypted runtime capability for disposable integration
    /// tests. This neither mints nor substitutes a tool-open confirmation.
    pub(crate) async fn fixture_credential(
        &self,
        agent: &layer2_core::AgentId,
    ) -> RuntimeCredential {
        let document = self
            .capabilities
            .find_one(doc! {"record.agent_id":agent.as_str(),"revoked":false})
            .await
            .unwrap()
            .unwrap();
        let record = records::read(&document).unwrap();
        RuntimeCredential::new(
            self.cipher
                .open(COLLECTION, record.id.as_bytes(), "bearer", record.secret)
                .unwrap(),
        )
    }
}

#[test]
fn capability_namespace_is_stable_across_runtime_restarts_but_not_agent_replacement() {
    let id = layer2_core::AgentId("agent_fixture".into());
    let original = records::identity(&id, "2026-09-15T00:00:00Z", &"a".repeat(64)).unwrap();
    assert_eq!(
        original,
        records::identity(&id, "2026-09-15T00:00:00Z", &"a".repeat(64)).unwrap()
    );
    assert_ne!(
        original,
        records::identity(&id, "2026-09-16T00:00:00Z", &"a".repeat(64)).unwrap()
    );
    assert_ne!(
        original,
        records::identity(&id, "2026-09-15T00:00:00Z", &"b".repeat(64)).unwrap()
    );
}

#[test]
fn capability_envelope_is_bound_to_record_and_redacts_raw_secret() {
    use base64::Engine;
    let key = AuthEncryptionKey::from_base64(
        &base64::engine::general_purpose::STANDARD.encode([19_u8; 32]),
    )
    .unwrap();
    let cipher = AuthCipher::new("fixture-deployment", &key).unwrap();
    let secret = format!("cma_ct_{}", "a".repeat(64));
    let envelope = cipher
        .seal(COLLECTION, b"capability-a", "bearer", &secret)
        .unwrap();
    assert!(!format!("{envelope:?}").contains(&secret));
    assert!(!serde_json::to_string(&envelope).unwrap().contains(&secret));
    assert!(cipher
        .open(COLLECTION, b"capability-b", "bearer", envelope.clone())
        .is_err());
    assert_eq!(
        cipher
            .open(COLLECTION, b"capability-a", "bearer", envelope)
            .unwrap(),
        secret
    );
}
