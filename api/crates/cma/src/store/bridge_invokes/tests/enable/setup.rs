use super::*;

// Setup control uses the definition validator and already imported custody.
// These management-only credential ports must not be called during enablement.
struct NoCredentialCalls;
#[async_trait::async_trait]
impl BridgeSetupCredentials for NoCredentialCalls {
    async fn choices(
        &self,
        _: &CallerContext,
        _: &str,
        _: &ResourceOwner,
    ) -> Result<BridgeConnectorChoices, BridgeError> {
        panic!("enablement must not discover credentials")
    }
    async fn status(
        &self,
        _: &BridgeService,
        _: &CallerContext,
        _: Option<&str>,
        _: &BridgeId,
        _: u64,
    ) -> Result<BridgeCredentialStatus, BridgeError> {
        panic!("enablement must not request credential management status")
    }
    async fn import(
        &self,
        _: &BridgeService,
        _: &CallerContext,
        _: Option<&str>,
        _: BridgeCredentialImport,
    ) -> Result<BridgeCredentialStatus, BridgeError> {
        panic!("enablement must not import credentials")
    }
    async fn revoke(
        &self,
        _: &BridgeService,
        _: &CallerContext,
        _: Option<&str>,
        _: BridgeCredentialRevocation,
    ) -> Result<BridgeCredentialStatus, BridgeError> {
        panic!("enablement must not revoke credentials")
    }
}
pub(super) fn service(f: &Fixture) -> BridgeSetupService {
    BridgeSetupService::new(
        f.definitions.clone(),
        BridgeSetupPolicy::new(
            "https://nyx.example".into(),
            "https://nyx.example".into(),
            "https://cma.example/api/v1/bridge-callbacks".into(),
        )
        .unwrap(),
        Arc::new(NoCredentialCalls),
    )
    .with_invoke_grants(Arc::new(f.service.clone()))
}
