use cmaeg_core::channel::Secret;
use ring::{
    hmac,
    rand::{SecureRandom, SystemRandom},
};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

use super::{store::NewRecord, Error, Gateway};

#[derive(Debug, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateAppRequest {
    pub app_id: u64,
    pub display_name: String,
    #[serde(default)]
    #[schema(write_only = true)]
    pub webhook_secret: Option<Secret>,
}

#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct CredentialStatus {
    pub present: bool,
    pub version: Option<u64>,
}

#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WebhookCredentialStatus {
    pub present: bool,
    pub version: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub previous: Option<WebhookCredentialOverlap>,
}

#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WebhookCredentialOverlap {
    pub version: u64,
    pub valid_until: i64,
}

#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AppIntegration {
    pub app_ref: String,
    pub host: String,
    pub app_id: u64,
    pub display_name: String,
    pub version: u64,
    pub status: String,
    pub webhook_url: String,
    pub webhook_credential: WebhookCredentialStatus,
    pub verification_credential: CredentialStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verification: Option<AppVerification>,
}

#[derive(Clone, Serialize, Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct AppVerification {
    pub verified_by: String,
    pub verified_at: i64,
    pub evidence_ref: String,
}

#[derive(Serialize, ToSchema)]
pub struct GitHubCapabilities {
    pub store_ready: bool,
    pub dynamic_app_registration: bool,
    pub native_app_webhook: bool,
    pub webhook_secret_rotation: bool,
    pub app_identity_verification: bool,
    pub ownership_verification: bool,
    pub automatic_reconciliation: bool,
}

#[derive(Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListAppsQuery {
    pub after: Option<String>,
    pub limit: Option<u32>,
}

#[derive(Serialize, ToSchema)]
pub struct AppList {
    pub items: Vec<AppIntegration>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct WebhookCredential {
    pub app_ref: String,
    pub version: u64,
    #[serde(serialize_with = "serialize_secret")]
    pub secret: Secret,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct AppOperation {
    pub response: AppIntegration,
    fingerprint_key: Vec<u8>,
    fingerprint: Vec<u8>,
}

impl AppOperation {
    pub fn new(response: AppIntegration, input: &[u8]) -> Result<Self, Error> {
        let mut fingerprint_key = vec![0; 32];
        SystemRandom::new()
            .fill(&mut fingerprint_key)
            .map_err(|_| Error::Unavailable)?;
        let fingerprint = hmac::sign(&hmac::Key::new(hmac::HMAC_SHA256, &fingerprint_key), input)
            .as_ref()
            .to_vec();
        Ok(Self {
            response,
            fingerprint_key,
            fingerprint,
        })
    }

    pub fn replay(self, input: &[u8]) -> Result<AppIntegration, Error> {
        hmac::verify(
            &hmac::Key::new(hmac::HMAC_SHA256, &self.fingerprint_key),
            input,
            &self.fingerprint,
        )
        .map_err(|_| Error::ManagementConflict)?;
        Ok(self.response)
    }
}

pub(super) fn validate_key(key: &str) -> Result<(), Error> {
    if key.is_empty() || key.len() > 200 || !key.bytes().all(|byte| byte.is_ascii_graphic()) {
        return Err(Error::Invalid);
    }
    Ok(())
}

fn serialize_secret<S: serde::Serializer>(
    secret: &Secret,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(secret.as_str())
}

impl CreateAppRequest {
    fn validate(&self) -> Result<(), Error> {
        if self.app_id == 0
            || self.display_name.trim().is_empty()
            || self.display_name.len() > 200
            || self.display_name.chars().any(char::is_control)
            || self
                .webhook_secret
                .as_ref()
                .is_some_and(|secret| !(16..=4096).contains(&secret.as_str().len()))
        {
            return Err(Error::Invalid);
        }
        Ok(())
    }

    fn fingerprint_input(&self) -> Result<Vec<u8>, Error> {
        serde_json::to_vec(&(
            "github-app-registration-v1",
            self.app_id,
            &self.display_name,
            self.webhook_secret.as_ref().map(Secret::as_str),
        ))
        .map_err(|_| Error::Unavailable)
    }
}

impl Gateway {
    pub(super) async fn list_apps(
        &self,
        owner: &str,
        query: ListAppsQuery,
    ) -> Result<AppList, Error> {
        let limit = query.limit.unwrap_or(50);
        if !(1..=100).contains(&limit)
            || query
                .after
                .as_deref()
                .is_some_and(|after| !super::config::valid_ref(after))
        {
            return Err(Error::Invalid);
        }
        let records = self
            .store
            .list_owner::<AppIntegration>(
                owner,
                "app_integration",
                query.after.as_deref(),
                limit + 1,
            )
            .await?;
        let mut items = Vec::new();
        for (id, version, integration) in records {
            if integration.app_ref != id || integration.version != version {
                return Err(Error::Unavailable);
            }
            items.push(integration);
        }
        let next_cursor = if items.len() > limit as usize {
            items.pop();
            items.last().map(|integration| integration.app_ref.clone())
        } else {
            None
        };
        Ok(AppList { items, next_cursor })
    }

    pub(super) async fn create_app(
        &self,
        owner: &str,
        idempotency_key: &str,
        public_url: &str,
        request: CreateAppRequest,
    ) -> Result<(AppIntegration, bool), Error> {
        request.validate()?;
        validate_key(idempotency_key)?;
        let operation_id = super::identity("app-registration", &(owner, idempotency_key));
        let input = request.fingerprint_input()?;
        match self.registration(owner, &operation_id, &input).await {
            Ok(integration) => return Ok((integration, false)),
            Err(Error::NotFound) => (),
            Err(error) => return Err(error),
        }
        let app_ref = format!("gha_{}", uuid::Uuid::new_v4().simple());
        let integration = AppIntegration {
            app_ref: app_ref.clone(),
            host: "github.com".into(),
            app_id: request.app_id,
            display_name: request.display_name,
            version: 1,
            status: "pending_verification".into(),
            webhook_url: format!(
                "{}/v1/github/apps/{app_ref}/webhooks",
                public_url.trim_end_matches('/')
            ),
            webhook_credential: WebhookCredentialStatus {
                present: request.webhook_secret.is_some(),
                version: request.webhook_secret.as_ref().map(|_| 1),
                previous: None,
            },
            verification_credential: CredentialStatus {
                present: false,
                version: None,
            },
            verification: None,
        };
        let registration = AppOperation::new(integration.clone(), &input)?;
        let mut records = vec![
            NewRecord::new(&operation_id, "app_registration", &registration)?,
            NewRecord::new(&app_ref, "app_integration", &integration)?,
        ];
        if let Some(secret) = request.webhook_secret {
            records.push(NewRecord::new(
                &super::identity("webhook-credential", &(&app_ref, 1)),
                "webhook_credential",
                &WebhookCredential {
                    app_ref: app_ref.clone(),
                    version: 1,
                    secret,
                },
            )?);
        }
        if self.store.create_many(owner, records).await.is_err() {
            return self
                .registration(owner, &operation_id, &input)
                .await
                .map(|integration| (integration, false))
                .map_err(|error| match error {
                    Error::NotFound => Error::Unavailable,
                    error => error,
                });
        }
        Ok((integration, true))
    }

    async fn registration(
        &self,
        owner: &str,
        id: &str,
        input: &[u8],
    ) -> Result<AppIntegration, Error> {
        let (stored_owner, registration) = self
            .store
            .read::<AppOperation>(id, "app_registration")
            .await?;
        if stored_owner != owner {
            return Err(Error::NotFound);
        }
        registration.replay(input)
    }

    pub(super) async fn read_app(
        &self,
        owner: &str,
        app_ref: &str,
    ) -> Result<AppIntegration, Error> {
        let (stored_owner, integration) = self.app_record(app_ref).await?;
        if stored_owner != owner {
            return Err(Error::NotFound);
        }
        Ok(integration)
    }

    async fn app_record(&self, app_ref: &str) -> Result<(String, AppIntegration), Error> {
        let (owner, version, integration) = self
            .store
            .read_versioned::<AppIntegration>(app_ref, "app_integration")
            .await?;
        if integration.app_ref != app_ref || integration.version != version {
            return Err(Error::Unavailable);
        }
        Ok((owner, integration))
    }

    pub(super) async fn webhook_credentials(
        &self,
        app_ref: &str,
    ) -> Result<(String, AppIntegration, Vec<Secret>), Error> {
        let (owner, integration) = self.app_record(app_ref).await?;
        if !integration.webhook_credential.present {
            return Err(Error::Signature);
        }
        let version = integration
            .webhook_credential
            .version
            .ok_or(Error::Unavailable)?;
        let mut secrets = vec![self.read_webhook_secret(&owner, app_ref, version).await?];
        if let Some(previous) = integration
            .webhook_credential
            .previous
            .as_ref()
            .filter(|previous| previous.valid_until > super::now())
        {
            secrets.push(
                self.read_webhook_secret(&owner, app_ref, previous.version)
                    .await?,
            );
        }
        Ok((owner, integration, secrets))
    }

    async fn read_webhook_secret(
        &self,
        owner: &str,
        app_ref: &str,
        version: u64,
    ) -> Result<Secret, Error> {
        let (credential_owner, credential) = self
            .store
            .read::<WebhookCredential>(
                &super::identity("webhook-credential", &(app_ref, version)),
                "webhook_credential",
            )
            .await
            .map_err(|_| Error::Unavailable)?;
        if credential_owner != owner
            || credential.app_ref != app_ref
            || credential.version != version
        {
            return Err(Error::Unavailable);
        }
        Ok(credential.secret)
    }
}
