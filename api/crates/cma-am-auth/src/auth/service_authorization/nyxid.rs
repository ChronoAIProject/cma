//! NyxID consent evidence belongs to the retained human and this OAuth client.
use super::*;
use std::collections::BTreeMap;

mod protocol;
use protocol::{Consent, ConsentRead, Discovery, Service};

pub(super) struct Nyxid {
    issuer: Url,
    api_origin: Url,
    management_url: Url,
}

impl Nyxid {
    pub(super) fn new(
        auth: &AuthState,
        config: &crate::service_config::ServiceAuthorizationConfig,
    ) -> Option<Self> {
        let browser = Url::parse(&auth.nyxid_public_url).ok()?;
        if browser.scheme() != "https"
            || !browser.username().is_empty()
            || browser.password().is_some()
            || browser.path() != "/"
            || browser.query().is_some()
            || browser.fragment().is_some()
        {
            return None;
        }
        Some(Self {
            issuer: config.issuer.clone(),
            api_origin: config.api_origin.clone(),
            management_url: browser.join("settings/consents").ok()?,
        })
    }

    fn client<'a>(&self, auth: &'a AuthState) -> Result<(&'a str, &'a str, &'a str), StoreError> {
        match &auth.mode {
            AuthMode::Provider {
                base_url,
                client_id,
                client_secret,
                ..
            } => Ok((base_url.trim_end_matches('/'), client_id, client_secret)),
            _ => Err(protocol::unavailable()),
        }
    }

    async fn discovery(&self, auth: &AuthState) -> Result<Discovery, StoreError> {
        let (base, _, _) = self.client(auth)?;
        let value: Discovery = protocol::read(
            auth.http
                .get(format!("{base}/.well-known/openid-configuration")),
        )
        .await?;
        value.validate(&self.issuer, &self.api_origin)?;
        Ok(value)
    }

    async fn token(
        &self,
        auth: &AuthState,
        subject: &SubjectId,
    ) -> Result<SecretValue, StoreError> {
        auth.actor_token_fenced(subject, None)
            .await
            .map_err(|_| protocol::unavailable())
    }

    async fn consent(
        &self,
        auth: &AuthState,
        token: &SecretValue,
    ) -> Result<ConsentRead, StoreError> {
        let (base, client, _) = self.client(auth)?;
        protocol::read_consent(
            auth.http
                .get(format!("{base}/api/v1/users/me/consents"))
                .bearer_auth(token.expose()),
            client,
        )
        .await
    }

    async fn evidence(
        &self,
        auth: &AuthState,
        intent: &RecoveryIntent,
    ) -> Result<(Option<Consent>, BTreeMap<ServiceId, Service>), StoreError> {
        let token = self.token(auth, &intent.subject).await?;
        self.evidence_with_token(auth, intent, &token).await
    }

    async fn evidence_with_token(
        &self,
        auth: &AuthState,
        intent: &RecoveryIntent,
        token: &SecretValue,
    ) -> Result<(Option<Consent>, BTreeMap<ServiceId, Service>), StoreError> {
        if intent.issuer != self.issuer.as_str() || intent.required.len() > 100 {
            return Err(protocol::unavailable());
        }
        let (base, _, _) = self.client(auth)?;
        let ConsentRead::Available(consent) = self.consent(auth, token).await? else {
            return Err(protocol::unavailable());
        };
        let services: protocol::Services = protocol::read(
            auth.http
                .get(format!("{base}/api/v1/user-services"))
                .bearer_auth(token.expose()),
        )
        .await?;
        Ok((
            consent,
            protocol::resolve(&intent.required, services.services, &self.api_origin)?,
        ))
    }
}

#[async_trait::async_trait]
impl GrantAuthority for Nyxid {
    fn issuer(&self) -> Option<&Url> {
        Some(&self.issuer)
    }

    fn authorization_origin(&self) -> Option<&Url> {
        Some(&self.api_origin)
    }

    fn management_url(&self) -> Option<Url> {
        Some(self.management_url.clone())
    }

    async fn account(
        &self,
        auth: &AuthState,
        subject: &SubjectId,
    ) -> Result<Option<VerifiedAccountGrant>, StoreError> {
        self.discovery(auth).await?;
        let token = self.token(auth, subject).await?;
        let ConsentRead::Available(consent) = self.consent(auth, &token).await? else {
            return Ok(Some(VerifiedAccountGrant {
                subject: subject.clone(),
                status: GrantStatus::Unknown,
                management_credential_unsupported: true,
                management_url: Some(self.management_url.clone()),
            }));
        };
        let status = match consent {
            None => GrantStatus::NotGranted,
            Some(grant) if !grant.live()? => GrantStatus::Expired,
            Some(grant) if grant.legacy_unrestricted => return Ok(None),
            Some(grant) if grant.allow_all_services => GrantStatus::Authorized,
            Some(_) => GrantStatus::Partial,
        };
        Ok(Some(VerifiedAccountGrant {
            subject: subject.clone(),
            status,
            management_credential_unsupported: false,
            management_url: Some(self.management_url.clone()),
        }))
    }

    async fn retirement_authority_version(
        &self,
        auth: &AuthState,
        token: &SecretValue,
    ) -> Result<Option<RetirementAuthorityVersion>, StoreError> {
        match self.consent(auth, token).await? {
            ConsentRead::Available(Some(consent)) => consent.retirement_authority_version(),
            ConsentRead::Available(None) | ConsentRead::UnsupportedCredential => Ok(None),
        }
    }

    async fn missing(
        &self,
        auth: &AuthState,
        intent: &RecoveryIntent,
    ) -> Result<Option<BTreeSet<ServiceId>>, StoreError> {
        let (consent, services) = self.evidence(auth, intent).await?;
        Ok(Some(protocol::missing(consent.as_ref(), &services)?))
    }

    async fn consent_url(
        &self,
        auth: &AuthState,
        intent: &RecoveryIntent,
        state: &str,
        s256_challenge: &str,
    ) -> Result<Option<Url>, StoreError> {
        let discovery = self.discovery(auth).await?;
        let (consent, services) = self.evidence(auth, intent).await?;
        if protocol::missing(consent.as_ref(), &services)? != intent.requested
            || intent.requested.is_empty()
            || !protocol::redirect_allowed(auth, &intent.redirect_uri)
        {
            return Ok(None);
        }
        let (_, client, _) = self.client(auth)?;
        let mut url =
            Url::parse(&discovery.authorization_endpoint).map_err(|_| protocol::unavailable())?;
        let live = consent
            .as_ref()
            .filter(|grant| grant.live().unwrap_or(false));
        {
            let mut query = url.query_pairs_mut();
            query.extend_pairs([
                ("response_type", "code"),
                ("client_id", client),
                ("redirect_uri", &intent.redirect_uri),
                ("state", state),
                ("code_challenge", s256_challenge),
                ("code_challenge_method", "S256"),
                ("prompt", "consent"),
            ]);
            if let Some(grant) = live {
                query.append_pair("scope", &grant.scopes);
                query.append_pair("service_access_mode", "incremental");
                query.append_pair("include_granted_scopes", "true");
                for id in &intent.requested {
                    query.append_pair("requested_service_ids", &services[id].id);
                }
            } else {
                query.append_pair("scope", "openid profile email roles proxy");
                for id in &intent.requested {
                    query.append_pair("resource", &services[id].resource_uri);
                }
            }
        }
        Ok(Some(url))
    }

    async fn redeem(
        &self,
        auth: &AuthState,
        intent: &RecoveryIntent,
        code: &str,
        verifier: &SecretValue,
    ) -> Result<Option<ConsentIdentity>, StoreError> {
        if intent.issuer != self.issuer.as_str()
            || !protocol::redirect_allowed(auth, &intent.redirect_uri)
        {
            return Ok(None);
        }
        let discovery = self.discovery(auth).await?;
        let (base, client, secret) = self.client(auth)?;
        let endpoint = super::super::rebase(&discovery.token_endpoint, base)
            .map_err(|_| protocol::unavailable())?;
        let response: protocol::Token = protocol::read(auth.http.post(endpoint).form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("client_id", client),
            ("client_secret", secret),
            ("redirect_uri", intent.redirect_uri.as_str()),
            ("code_verifier", verifier.expose()),
        ]))
        .await?;
        let token = SecretValue::new(response.access_token);
        if token.expose().is_empty() || token.expose().len() > 64 * 1024 {
            return Ok(None);
        }
        let endpoint = super::super::rebase(&discovery.introspection_endpoint, base)
            .map_err(|_| protocol::unavailable())?;
        let verified: protocol::Introspection = protocol::read(auth.http.post(endpoint).form(&[
            ("token", token.expose()),
            ("token_type_hint", "access_token"),
            ("client_id", client),
            ("client_secret", secret),
        ]))
        .await?;
        if !verified.matches(&self.issuer, client, &intent.subject) {
            return Ok(None);
        }
        // Read back with the exchanged credential; never install it in the original login.
        let (grant, services) = self.evidence_with_token(auth, intent, &token).await?;
        if grant.is_none() {
            return Ok(None);
        }
        Ok(Some(ConsentIdentity {
            subject: intent.subject.clone(),
            issuer: intent.issuer.clone(),
            missing: protocol::missing(grant.as_ref(), &services)?,
        }))
    }
}
