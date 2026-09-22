use super::*;

const ENDPOINT: &str = "https://gateway.example/tools";

#[tokio::test]
async fn tool_endpoint_survives_replay_and_cannot_change_under_a_retained_binding() {
    let f = fixture();
    let binding = f.ready_with_tool_endpoint(Some(ENDPOINT.into())).await;
    let retained: ProviderBinding =
        serde_json::from_value(serde_json::to_value(&binding).unwrap()).unwrap();
    assert_eq!(retained, binding);
    assert!(binding
        .honoured
        .iter()
        .any(|field| field == "tools.native_mcp_url"));
    assert!(!binding
        .refused
        .iter()
        .any(|refusal| refusal.field == "tools.native_mcp_url"));
    let profile = profile("Original instruction");
    let digest = format!("sha256:{}", content_digest(&profile).unwrap());
    let replayed = f
        .service
        .put_with_tool_endpoint(
            &f.caller,
            "binding-one",
            profile.clone(),
            &digest,
            &f.key,
            Some(ENDPOINT.into()),
        )
        .await
        .unwrap();
    assert_eq!(replayed, binding);
    for changed in [None, Some("https://another.example/tools".into())] {
        assert!(matches!(
            f.service
                .put_with_tool_endpoint(
                    &f.caller,
                    "binding-one",
                    profile.clone(),
                    &digest,
                    &f.key,
                    changed,
                )
                .await,
            Err(BindingError::Conflict)
        ));
        assert_eq!(f.binding().await, binding);
    }
    let mut changed = binding.clone();
    changed.tool_endpoint = Some("https://another.example/tools".into());
    changed.revision += 1;
    assert!(f
        .store
        .save_binding(Some(binding.revision), changed)
        .await
        .is_err());
}

#[tokio::test]
async fn supplied_key_rotation_retains_the_exact_tool_destination() {
    let f = fixture();
    let binding = f.ready_with_tool_endpoint(Some(ENDPOINT.into())).await;
    let sandbox = f
        .store
        .get_sandbox(&binding.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(sandbox.credential.source, binding.credential_source());
    let new_key =
        ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
    let next = binding
        .rotate_credential(new_key.sha256(), crate::now_rfc3339())
        .unwrap();
    assert_eq!(next.tool_endpoint, binding.tool_endpoint);
    let mut authority = sandbox.credential.current.clone().unwrap();
    authority.generation += 1;
    authority.provider_key_id = format!("provided-{}", new_key.sha256());
    let delivery = next
        .admit_credential_delivery(&sandbox, authority.clone())
        .unwrap();
    assert_eq!(delivery.credential.source, sandbox.credential.source);
    assert_eq!(next.generation, binding.generation);
    let mut another = sandbox.clone();
    another.credential.source = SandboxCredentialSource::Binding {
        binding_id: binding.binding_id.clone(),
        tool_endpoint: Some("https://another.example/tools".into()),
    };
    another.revision += 1;
    assert!(next.admit_credential_delivery(&another, authority).is_err());
    assert!(f
        .store
        .save_sandbox(sandbox.revision, another)
        .await
        .is_err());
}

#[test]
fn old_credential_authority_records_keep_their_original_serialization() {
    let old = json!({"kind":"binding","binding_id":"existing-binding"});
    let source: SandboxCredentialSource = serde_json::from_value(old.clone()).unwrap();
    assert!(source.validate());
    assert_eq!(serde_json::to_value(source).unwrap(), old);
}
