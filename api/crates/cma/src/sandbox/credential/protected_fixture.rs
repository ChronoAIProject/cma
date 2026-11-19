use super::*;
use layer2_core::sandbox::store::{
    ClaimedWork, WorkAuthority, WorkAuthorityProtocol, WorkLeaseGrant,
};

impl MemoryCustody {
    pub(super) fn seed_retained(
        &self,
        material: CredentialMaterial,
    ) -> Result<(), CredentialCustodyError> {
        let key = (material.sandbox_id.clone(), material.authority.generation);
        let mut values = self.values.lock().unwrap();
        if values.contains_key(&key) {
            return Err(CredentialCustodyError::Duplicate);
        }
        values.insert(key, material);
        Ok(())
    }
}

pub(super) async fn protected_issue(
    controller: &ManagedSandboxCredentials,
    sandbox: &Sandbox,
    generation: u64,
) -> Result<CredentialMaterial, CredentialError> {
    let mut claim = sandbox.clone();
    let now = SystemTime::now();
    let stamp = rfc3339_at(now);
    claim.work = SandboxWorkState::new(SandboxWorkStep::MintInitialCredential, stamp.clone());
    claim.work.authority_protocol = Some(WorkAuthorityProtocol::RenewedV1);
    claim
        .work
        .claim(
            "credential-test".into(),
            &stamp,
            rfc3339_at(now + std::time::Duration::from_secs(15)),
        )
        .unwrap();
    WorkAuthority::new(
        ClaimedWork::Sandbox(Box::new(claim.clone())),
        tokio::time::Instant::now(),
    )
    .run(
        async { Ok(controller.issue(&claim, generation, now).await) },
        || async {
            WorkLeaseGrant::new(
                rfc3339_at(SystemTime::now() + std::time::Duration::from_secs(15)),
                15_000,
            )
            .map(Some)
        },
    )
    .await
    .unwrap()
}
