use cmaeg_core::channel::Secret;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::{
    integrations::{
        validate_key, AppOperation, AppVerification, WebhookCredential, WebhookCredentialOverlap,
    },
    store::NewRecord,
    AppIntegration, Error, Gateway,
};

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdateAppRequest {
    pub expected_version: u64,
    pub display_name: String,
}

#[derive(Deserialize, Serialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct VerifyAppRequest {
    pub expected_version: u64,
    pub evidence_ref: String,
}

#[derive(Deserialize, ToSchema)]
#[serde(deny_unknown_fields)]
pub struct WebhookSecretRequest {
    pub expected_version: u64,
    #[schema(write_only = true)]
    pub webhook_secret: Secret,
    #[serde(default = "default_overlap")]
    #[schema(default = 600, minimum = 0, maximum = 3600)]
    pub overlap_seconds: u32,
}

fn default_overlap() -> u32 {
    600
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct AppIdentity {
    host: String,
    app_id: u64,
    app_ref: String,
}

pub(super) enum AppMutation {
    Metadata(UpdateAppRequest),
    Verification {
        request: VerifyAppRequest,
        verifier: String,
    },
    WebhookSecret(WebhookSecretRequest),
}

impl AppMutation {
    fn kind(&self) -> &'static str {
        match self {
            Self::Metadata(_) => "metadata",
            Self::Verification { .. } => "verification",
            Self::WebhookSecret(_) => "webhook-secret",
        }
    }

    fn expected_version(&self) -> u64 {
        match self {
            Self::Metadata(request) => request.expected_version,
            Self::Verification { request, .. } => request.expected_version,
            Self::WebhookSecret(request) => request.expected_version,
        }
    }

    fn input(&self) -> Result<Vec<u8>, Error> {
        match self {
            Self::Metadata(request) => {
                if request.expected_version == 0
                    || request.display_name.trim().is_empty()
                    || request.display_name.len() > 200
                    || request.display_name.chars().any(char::is_control)
                {
                    return Err(Error::Invalid);
                }
                serde_json::to_vec(request).map_err(|_| Error::Unavailable)
            }
            Self::Verification { request, .. } => {
                if request.expected_version == 0
                    || request.evidence_ref.trim().is_empty()
                    || request.evidence_ref.len() > 512
                    || request.evidence_ref.chars().any(char::is_control)
                {
                    return Err(Error::Invalid);
                }
                serde_json::to_vec(request).map_err(|_| Error::Unavailable)
            }
            Self::WebhookSecret(request) => {
                if request.expected_version == 0
                    || !(16..=4096).contains(&request.webhook_secret.as_str().len())
                    || request.overlap_seconds > 3600
                {
                    return Err(Error::Invalid);
                }
                serde_json::to_vec(&(
                    request.expected_version,
                    request.webhook_secret.as_str(),
                    request.overlap_seconds,
                ))
                .map_err(|_| Error::Unavailable)
            }
        }
    }
}

impl Gateway {
    pub(super) async fn mutate_app(
        &self,
        owner: &str,
        app_ref: &str,
        key: &str,
        mutation: AppMutation,
    ) -> Result<AppIntegration, Error> {
        validate_key(key)?;
        let input = mutation.input()?;
        let mut app = self.read_app(owner, app_ref).await?;
        let operation_id =
            super::identity("app-operation", &(owner, app_ref, mutation.kind(), key));
        match self.app_operation(owner, &operation_id, &input).await {
            Ok(app) => return Ok(app),
            Err(Error::NotFound) => (),
            Err(error) => return Err(error),
        }
        let expected_version = mutation.expected_version();
        if app.version != expected_version {
            return Err(Error::VersionConflict(app.version));
        }
        app.version = app.version.checked_add(1).ok_or(Error::Unavailable)?;
        let mut records = Vec::new();
        let verifying = matches!(mutation, AppMutation::Verification { .. });
        match mutation {
            AppMutation::Metadata(request) => app.display_name = request.display_name,
            AppMutation::Verification { request, verifier } => {
                if !self.check_app_identity(&app).await? {
                    records.push(NewRecord::new(
                        &super::identity("app-identity", &(&app.host, app.app_id)),
                        "app_identity",
                        &AppIdentity {
                            host: app.host.clone(),
                            app_id: app.app_id,
                            app_ref: app.app_ref.clone(),
                        },
                    )?);
                }
                app.status = "verified".into();
                app.verification = Some(AppVerification {
                    verified_by: verifier,
                    verified_at: super::now(),
                    evidence_ref: request.evidence_ref,
                });
            }
            AppMutation::WebhookSecret(request) => {
                let now = super::now();
                if request.overlap_seconds > 0
                    && app
                        .webhook_credential
                        .previous
                        .as_ref()
                        .is_some_and(|previous| previous.valid_until > now)
                {
                    return Err(Error::ManagementConflict);
                }
                app.webhook_credential.previous = app
                    .webhook_credential
                    .version
                    .filter(|_| request.overlap_seconds > 0)
                    .map(|version| WebhookCredentialOverlap {
                        version,
                        valid_until: now + i64::from(request.overlap_seconds),
                    });
                let version = app
                    .webhook_credential
                    .version
                    .unwrap_or(0)
                    .checked_add(1)
                    .ok_or(Error::Unavailable)?;
                records.push(NewRecord::new(
                    &super::identity("webhook-credential", &(app_ref, version)),
                    "webhook_credential",
                    &WebhookCredential {
                        app_ref: app_ref.into(),
                        version,
                        secret: request.webhook_secret,
                    },
                )?);
                app.webhook_credential.present = true;
                app.webhook_credential.version = Some(version);
            }
        }
        let operation = AppOperation::new(app.clone(), &input)?;
        records.push(NewRecord::new(&operation_id, "app_operation", &operation)?);
        if let Err(error) = self
            .store
            .update_integration(owner, expected_version, &app, records)
            .await
        {
            return match self.app_operation(owner, &operation_id, &input).await {
                Err(Error::NotFound) => {
                    if verifying {
                        self.check_app_identity(&app).await?;
                    }
                    Err(error)
                }
                result => result,
            };
        }
        Ok(app)
    }

    async fn check_app_identity(&self, app: &AppIntegration) -> Result<bool, Error> {
        let id = super::identity("app-identity", &(&app.host, app.app_id));
        match self.store.read::<AppIdentity>(&id, "app_identity").await {
            Ok((_, identity))
                if identity.host == app.host
                    && identity.app_id == app.app_id
                    && identity.app_ref == app.app_ref =>
            {
                Ok(true)
            }
            Ok(_) => Err(Error::ManagementConflict),
            Err(Error::NotFound) => Ok(false),
            Err(error) => Err(error),
        }
    }

    async fn app_operation(
        &self,
        owner: &str,
        id: &str,
        input: &[u8],
    ) -> Result<AppIntegration, Error> {
        let (stored_owner, operation) =
            self.store.read::<AppOperation>(id, "app_operation").await?;
        if stored_owner != owner {
            return Err(Error::NotFound);
        }
        operation.replay(input)
    }
}
