//! Enabled-only composition for the sponsorship authority and private gateway.

mod adapters;
mod worker;

use std::future::Future;
use std::sync::Arc;

use axum::Router;
use layer2_core::{
    SponsoredBillingProvider, SponsoredCredentialProvider, SponsoredModelDataPlaneService,
    SponsoredModelProvider, SponsoredPricingProvider, SponsorshipAuditPseudonymizer,
    SponsorshipService, SponsorshipStore, SponsorshipStoreError, SystemSponsorshipClock,
    TriggerValidationError,
};
use layer3_adapter::SponsoredModelGatewayOrigin;
use mongodb::Client;

use self::adapters::{
    HttpSponsoredBillingProvider, HttpSponsoredCredentialProvider, HttpSponsoredPricingProvider,
};
use self::worker::{
    SponsorshipWorkerError, SponsorshipWorkerPlan, SponsorshipWorkerSettings, SponsorshipWorkers,
};
use crate::auth::AuthState;
use crate::config::SponsorshipRuntimeConfig;
use crate::model_gateway::SponsoredModelGateway;
use crate::model_provider::HttpSponsoredModelProvider;

/// Configuration retained only when sponsorship is enabled for this process.
#[derive(Debug)]
pub(crate) struct EnabledSponsorshipConfig {
    runtime: SponsorshipRuntimeConfig,
    gateway_origin: SponsoredModelGatewayOrigin,
}

/// One completely composed sponsorship authority.
pub(crate) struct SponsorshipRuntime {
    service: Arc<SponsorshipService>,
    gateway_origin: SponsoredModelGatewayOrigin,
    gateway: Router,
    workers: Option<SponsorshipWorkerPlan>,
}

impl SponsorshipRuntime {
    pub(crate) fn service(&self) -> Arc<SponsorshipService> {
        Arc::clone(&self.service)
    }

    pub(crate) fn gateway_origin(&self) -> SponsoredModelGatewayOrigin {
        self.gateway_origin.clone()
    }

    pub(crate) fn gateway_router(&self) -> Router {
        self.gateway.clone()
    }

    /// Start the prevalidated recovery lanes once startup has committed.
    pub(crate) fn start_workers(
        &mut self,
        shutdown: tokio::sync::watch::Sender<bool>,
    ) -> Option<SponsorshipWorkers> {
        self.workers.take().map(|workers| workers.start(shutdown))
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SponsorshipCompositionError {
    #[error(
        "CHRONO_SERVER_PUBLIC_URL must be one exact canonical HTTPS origin when CHRONO_SPONSORSHIP_ENABLED=true"
    )]
    GatewayOrigin,
    #[error("the sponsorship recovery worker is not usable: {0}")]
    Worker(#[from] SponsorshipWorkerError),
    #[error("the sponsorship audit authority is not usable: {0}")]
    Audit(TriggerValidationError),
    #[error("the sponsorship credential-provider adapter is not usable: {0}")]
    CredentialProvider(String),
    #[error("the sponsorship billing adapter is not usable: {0}")]
    BillingProvider(String),
    #[error("the sponsorship pricing adapter is not usable: {0}")]
    PricingProvider(String),
    #[error("the sponsored model provider adapter is not usable: {0}")]
    ModelProvider(String),
    #[error("the sponsorship store is not usable: {0}")]
    Store(#[from] SponsorshipStoreError),
}

/// Drop absent or staged-disabled secrets before any runtime dependency exists.
///
/// The origin result is intentionally opaque on failure. Its parser uses only
/// `CHRONO_SERVER_PUBLIC_URL`, but retaining an arbitrary parser message here
/// would make this composition boundary capable of reflecting configuration.
pub(crate) fn select(
    runtime: Option<SponsorshipRuntimeConfig>,
    gateway_origin: Result<Option<SponsoredModelGatewayOrigin>, String>,
) -> Result<Option<EnabledSponsorshipConfig>, SponsorshipCompositionError> {
    let Some(runtime) = runtime else {
        return Ok(None);
    };
    if !runtime.gate.enabled {
        return Ok(None);
    }
    let gateway_origin = gateway_origin
        .map_err(|_| SponsorshipCompositionError::GatewayOrigin)?
        .ok_or(SponsorshipCompositionError::GatewayOrigin)?;
    Ok(Some(EnabledSponsorshipConfig {
        runtime,
        gateway_origin,
    }))
}

/// Compose nothing for dormant/staged deployments and never return a partial
/// enabled runtime.
pub(crate) async fn compose(
    selected: Option<EnabledSponsorshipConfig>,
    client: &Client,
    database: &str,
    deployment: &str,
    auth: Arc<AuthState>,
    sessions: Arc<layer2_core::SessionService>,
) -> Result<Option<SponsorshipRuntime>, SponsorshipCompositionError> {
    compose_selected_with(selected, |selected| {
        compose_enabled(selected, client, database, deployment, auth, sessions)
    })
    .await
}

async fn compose_selected_with<T, Compose, Composed>(
    selected: Option<EnabledSponsorshipConfig>,
    compose_enabled: Compose,
) -> Result<Option<T>, SponsorshipCompositionError>
where
    Compose: FnOnce(EnabledSponsorshipConfig) -> Composed,
    Composed: Future<Output = Result<T, SponsorshipCompositionError>>,
{
    let Some(selected) = selected else {
        return Ok(None);
    };
    compose_enabled(selected).await.map(Some)
}

struct LocalDependencies {
    provider: Arc<dyn SponsoredCredentialProvider>,
    billing: Arc<dyn SponsoredBillingProvider>,
    pricing: Arc<dyn SponsoredPricingProvider>,
    model_provider: Arc<dyn SponsoredModelProvider>,
    audit: Arc<SponsorshipAuditPseudonymizer>,
    worker: SponsorshipWorkerSettings,
}

fn prepare_local_dependencies(
    runtime: &SponsorshipRuntimeConfig,
) -> Result<LocalDependencies, SponsorshipCompositionError> {
    let worker = SponsorshipWorkerSettings::new(
        runtime.worker.interval,
        runtime.worker.page_size,
        runtime.worker.max_pages,
        runtime.worker.concurrency,
    )?;
    let audit = Arc::new(
        SponsorshipAuditPseudonymizer::new(runtime.secrets.audit_hmac_key.as_bytes().to_vec())
            .map_err(SponsorshipCompositionError::Audit)?,
    );

    let provider_adapter_id = runtime.adapters.provider_adapter_id.clone();
    let provider_base_url = runtime.adapters.provider_base_url.clone();
    let provider = Arc::new(
        HttpSponsoredCredentialProvider::new(
            provider_adapter_id.clone(),
            provider_base_url.clone(),
        )
        .map_err(SponsorshipCompositionError::CredentialProvider)?,
    ) as Arc<dyn SponsoredCredentialProvider>;
    let billing = Arc::new(
        HttpSponsoredBillingProvider::new(
            runtime.adapters.billing_adapter_id.clone(),
            runtime.adapters.billing_base_url.clone(),
        )
        .map_err(SponsorshipCompositionError::BillingProvider)?,
    ) as Arc<dyn SponsoredBillingProvider>;
    let pricing = Arc::new(
        HttpSponsoredPricingProvider::new(
            runtime.adapters.pricing_adapter_id.clone(),
            runtime.adapters.pricing_base_url.clone(),
        )
        .map_err(SponsorshipCompositionError::PricingProvider)?,
    ) as Arc<dyn SponsoredPricingProvider>;
    let model_provider = Arc::new(
        HttpSponsoredModelProvider::new(provider_adapter_id, provider_base_url)
            .map_err(SponsorshipCompositionError::ModelProvider)?,
    ) as Arc<dyn SponsoredModelProvider>;

    Ok(LocalDependencies {
        provider,
        billing,
        pricing,
        model_provider,
        audit,
        worker,
    })
}

async fn compose_enabled(
    selected: EnabledSponsorshipConfig,
    client: &Client,
    database: &str,
    deployment: &str,
    auth: Arc<AuthState>,
    sessions: Arc<layer2_core::SessionService>,
) -> Result<SponsorshipRuntime, SponsorshipCompositionError> {
    // Finish every fallible local constructor before Mongo changes its durable
    // fence or indexes. After the store opens, only infallible assembly and the
    // worker's all-or-nothing lane validation remain.
    let local = prepare_local_dependencies(&selected.runtime)?;
    let SponsorshipRuntimeConfig {
        gate,
        fence,
        secrets,
        ..
    } = selected.runtime;
    let opened = crate::store::open_sponsorship(
        client,
        database,
        deployment,
        secrets.encryption_keyring.as_str(),
        secrets.operational_keyring.as_str(),
        fence,
    )
    .await?;
    drop(secrets);

    let store: Arc<dyn SponsorshipStore> = opened.store;
    let service = Arc::new(SponsorshipService::new(
        gate,
        Arc::clone(&store),
        local.provider,
        local.billing,
        local.pricing,
        Arc::new(SystemSponsorshipClock),
        local.audit,
        opened.operational,
    ));
    let data_plane = Arc::new(SponsoredModelDataPlaneService::new(
        Arc::clone(&service),
        Arc::clone(&store),
        local.model_provider,
    ));
    let gateway_origin = selected.gateway_origin;
    let gateway = SponsoredModelGateway::new(
        auth,
        sessions,
        Arc::clone(&data_plane),
        gateway_origin.clone(),
    )
    .router();

    let workers = worker::prepare(
        Arc::clone(&service),
        store,
        data_plane,
        local.worker,
        opened.retiring_encryption_key_ids,
    )?;

    Ok(SponsorshipRuntime {
        service,
        gateway_origin,
        gateway,
        workers: Some(workers),
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use layer2_core::{
        SponsoredQuota, SponsorshipGate, SponsorshipGateFence, SponsorshipRuntimeLimits,
    };
    use zeroize::Zeroizing;

    use super::*;
    use crate::config::{
        SponsorshipAdaptersConfig, SponsorshipSecretsConfig, SponsorshipWorkerConfig,
    };

    const SECRET_MARKER: &str = "composition-secret-must-never-appear";

    fn runtime_config(enabled: bool) -> SponsorshipRuntimeConfig {
        let runtime_limits = SponsorshipRuntimeLimits {
            default_policy_expiry_seconds: 900,
            maximum_policy_expiry_seconds: 3_600,
            maximum_active_policies_per_sponsor: 10,
            launch_rate_window_seconds: 60,
            maximum_launches_per_sponsor_per_window: 20,
            maximum_per_launch_quota: SponsoredQuota {
                requests: 10,
                input_tokens: 10_000,
                output_tokens: 5_000,
            },
            maximum_policy_quota: SponsoredQuota {
                requests: 100,
                input_tokens: 100_000,
                output_tokens: 50_000,
            },
            maximum_requests_per_minute: 30,
            maximum_per_launch_budget_micros: 100_000,
            maximum_policy_budget_micros: 1_000_000,
            maximum_concurrent_launches: 5,
        };
        let gate = SponsorshipGate {
            enabled,
            expiry_evidence_id: Some("expiry-evidence-671".into()),
            revocation_evidence_id: Some("revocation-evidence-671".into()),
            trusted_provider_policy_key: "provider-policy-v1".into(),
            allowed_provider_policy_keys: BTreeSet::from(["provider-policy-v1".into()]),
            runtime_limits: runtime_limits.clone(),
            kill_switch_engaged: !enabled,
        };
        let fence = SponsorshipGateFence {
            configuration_digest: gate.configuration_digest().unwrap(),
            epoch: 1,
            kill_switch_engaged: !enabled,
            trusted_provider_policy_key: gate.trusted_provider_policy_key.clone(),
            runtime_limits,
        };
        SponsorshipRuntimeConfig {
            gate,
            fence,
            adapters: SponsorshipAdaptersConfig {
                provider_adapter_id: "provider-adapter-v1".into(),
                provider_base_url: "https://provider.example".into(),
                billing_adapter_id: "billing-adapter-v1".into(),
                billing_base_url: "https://billing.example".into(),
                pricing_adapter_id: "pricing-adapter-v1".into(),
                pricing_base_url: "https://pricing.example".into(),
            },
            worker: SponsorshipWorkerConfig {
                interval: std::time::Duration::from_secs(10),
                page_size: 20,
                max_pages: 4,
                concurrency: 2,
            },
            secrets: SponsorshipSecretsConfig {
                encryption_keyring: Zeroizing::new(SECRET_MARKER.into()),
                operational_keyring: Zeroizing::new(SECRET_MARKER.into()),
                audit_hmac_key: Zeroizing::new("a".repeat(32)),
            },
        }
    }

    fn gateway_origin() -> SponsoredModelGatewayOrigin {
        SponsoredModelGatewayOrigin::parse("https://cma.example").unwrap()
    }

    #[tokio::test]
    async fn absent_and_staged_disabled_modes_invoke_no_runtime_constructor() {
        let constructor_calls = AtomicUsize::new(0);
        for selected in [
            select(None, Err(SECRET_MARKER.into())).unwrap(),
            select(Some(runtime_config(false)), Err(SECRET_MARKER.into())).unwrap(),
        ] {
            let composed = compose_selected_with(selected, |_| {
                constructor_calls.fetch_add(1, Ordering::SeqCst);
                std::future::ready(Ok::<_, SponsorshipCompositionError>(()))
            })
            .await
            .unwrap();
            assert!(composed.is_none());
        }
        assert_eq!(constructor_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn enabled_invalid_origin_fails_before_dependency_construction() {
        let dependency_calls = AtomicUsize::new(0);
        let selected = select(
            Some(runtime_config(true)),
            Err(format!("invalid origin containing {SECRET_MARKER}")),
        );
        if selected.is_ok() {
            dependency_calls.fetch_add(1, Ordering::SeqCst);
        }
        let error = selected.expect_err("the invalid enabled origin fails selection");

        assert!(matches!(error, SponsorshipCompositionError::GatewayOrigin));
        assert_eq!(dependency_calls.load(Ordering::SeqCst), 0);
        assert!(!format!("{error}").contains(SECRET_MARKER));
        assert!(!format!("{error:?}").contains(SECRET_MARKER));
    }

    #[tokio::test]
    async fn enabled_dependency_errors_return_no_partial_runtime() {
        let selected = select(Some(runtime_config(true)), Ok(Some(gateway_origin()))).unwrap();
        let error = compose_selected_with(selected, |_| {
            std::future::ready(Err::<(), _>(SponsorshipCompositionError::ModelProvider(
                "deliberate constructor failure".into(),
            )))
        })
        .await
        .expect_err("the constructor failure is propagated");

        assert!(matches!(
            error,
            SponsorshipCompositionError::ModelProvider(_)
        ));
    }

    #[test]
    fn worker_and_secret_failures_are_validated_locally_and_redacted() {
        let mut invalid_worker = runtime_config(true);
        invalid_worker.worker.concurrency = 0;
        assert!(matches!(
            prepare_local_dependencies(&invalid_worker),
            Err(SponsorshipCompositionError::Worker(
                SponsorshipWorkerError::InvalidConfiguration(_)
            ))
        ));

        let mut invalid_audit = runtime_config(true);
        let invalid_audit_key = format!("{SECRET_MARKER}{}", "x".repeat(1_024));
        invalid_audit.secrets.audit_hmac_key = Zeroizing::new(invalid_audit_key);
        assert!(!format!("{invalid_audit:?}").contains(SECRET_MARKER));
        let error = prepare_local_dependencies(&invalid_audit)
            .err()
            .expect("the undersized audit key fails local construction");
        assert!(matches!(error, SponsorshipCompositionError::Audit(_)));
        assert!(!format!("{error}").contains(SECRET_MARKER));
        assert!(!format!("{error:?}").contains(SECRET_MARKER));
    }
}
