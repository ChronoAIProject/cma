//! Explicit consent is tied to one successfully consumed PKCE attempt/browser.
use super::*;
use crate::auth::{persistence::LoginAttempt, session_routes::request_cookie, AUTH_COOKIE};
use axum::http::{header, HeaderMap};

impl AuthState {
    pub(in crate::auth) async fn actor_login_intent(
        &self,
        link_id: &str,
    ) -> Result<Option<ActorLoginIntent>, BotError> {
        let row = self.link_record(link_id).await?;
        if row.active || row.revoked || row.subject.is_some() || row.expires_at <= now() {
            return Err(BotError::Forbidden);
        }
        Ok(
            (row.activation == ActorActivation::BrowserConsent).then_some(ActorLoginIntent {
                link_id: row.link_id,
                actor_key: row.actor_key,
                revision: row.revision,
            }),
        )
    }

    /// Called only after PKCE and initiating-browser verification succeeds.
    pub(in crate::auth) async fn retain_actor_login_completion(
        &self,
        attempt: &LoginAttempt,
        browser_cookie: &str,
        subject: &SubjectId,
    ) -> Result<(), BotError> {
        let Some(intent) = &attempt.actor_link_intent else {
            return Ok(());
        };
        if attempt.expires_at <= SystemTime::now() {
            return Err(BotError::Forbidden);
        }
        let mut row = self.link_record(&intent.link_id).await?;
        if row.activation != ActorActivation::BrowserConsent
            || row.actor_key != intent.actor_key
            || row.revision != intent.revision
            || row.active
            || row.revoked
            || row.subject.is_some()
            || row.expires_at <= now()
        {
            return Err(BotError::Forbidden);
        }
        let state = self
            .store
            .subject(subject)
            .await
            .map_err(|_| BotError::Unavailable)?
            .ok_or(BotError::Forbidden)?;
        self.actor_token_fenced(subject, Some(&state.incarnation))
            .await?;
        let revision = row.revision;
        row.subject = Some(subject.clone());
        row.incarnation = Some(state.incarnation);
        row.profile = state.profile;
        row.revision += 1;
        row.browser_consent = Some(ActorBrowserConsent {
            attempt_digest: attempt
                .id
                .as_bytes()
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect(),
            browser_digest: digest("actor-browser-consent", browser_cookie),
            csrf_token: uuid::Uuid::new_v4().simple().to_string(),
            review_revision: row.revision,
            approved_at: None,
        });
        if !self
            .store
            .replace_actor_link(Some(revision), row)
            .await
            .map_err(|_| BotError::Unavailable)?
        {
            return Err(BotError::Conflict);
        }
        Ok(())
    }

    pub(super) async fn actor_browser_review(
        &self,
        link_id: &str,
        headers: &HeaderMap,
    ) -> Result<ActorLinkRecord, BotError> {
        if headers.contains_key(header::AUTHORIZATION) {
            return Err(BotError::Forbidden);
        }
        let cookie = request_cookie(headers, AUTH_COOKIE).ok_or(BotError::Forbidden)?;
        let login = self
            .store
            .browser_login(&SecretDigest::identify(
                crate::auth::AUTH_COOKIE_DOMAIN,
                cookie,
            ))
            .await
            .map_err(|_| BotError::Unavailable)?
            .filter(|login| login.expires_at > SystemTime::now())
            .ok_or(BotError::Forbidden)?;
        let row = self.link_record(link_id).await?;
        let evidence = row.browser_consent.as_ref().ok_or(BotError::Forbidden)?;
        if row.activation != ActorActivation::BrowserConsent
            || row.revoked
            || (!row.active && row.expires_at <= now())
            || row.subject.as_ref() != Some(&login.subject)
            || evidence.browser_digest != digest("actor-browser-consent", cookie)
        {
            return Err(BotError::Forbidden);
        }
        self.actor_token_fenced(&login.subject, row.incarnation.as_deref())
            .await?;
        let latest = self.link_record(link_id).await?;
        if latest.revoked
            || (latest.revision != row.revision && !latest.same_active_authority(&row))
        {
            return Err(BotError::Conflict);
        }
        Ok(row)
    }

    pub(super) async fn approve_actor_browser_consent(
        &self,
        link_id: &str,
        headers: &HeaderMap,
        csrf: &str,
        subject: &str,
        review_revision: u64,
    ) -> Result<ActorGrant, BotError> {
        if !self.permits_browser_origin(headers) {
            return Err(BotError::Forbidden);
        }
        for _ in 0..4 {
            let mut row = match self.actor_browser_review(link_id, headers).await {
                Err(BotError::Conflict) => continue,
                result => result?,
            };
            let consent = row.browser_consent.as_ref().ok_or(BotError::Forbidden)?;
            if !opaque(csrf)
                || consent.csrf_token != csrf
                || consent.review_revision != review_revision
                || row.subject.as_ref().map(SubjectId::as_str) != Some(subject)
            {
                return Err(BotError::Forbidden);
            }
            if row.active {
                return row.grant();
            }
            if row
                .origin
                .as_ref()
                .is_some_and(|origin| origin.reply_expires_at <= now())
            {
                return Err(BotError::Forbidden);
            }
            let revision = row.revision;
            let consent = row.browser_consent.as_mut().ok_or(BotError::Forbidden)?;
            consent.approved_at = Some(now());
            row.active = true;
            row.renewed_at = now();
            row.revision += 1;
            // Grant activation and discoverable continuation are one durable CAS.
            if row.origin.is_some() {
                row.wakeup = Some(ActorWakeup {
                    notification_id: digest(
                        "actor-authorized-notification",
                        &format!("{}:{}", row.link_id, consent.attempt_digest),
                    ),
                    state: ActorWakeupState::Pending { retry_at: now() },
                });
            }
            if self
                .store
                .replace_actor_link(Some(revision), row.clone())
                .await
                .map_err(|_| BotError::Unavailable)?
            {
                return row.grant();
            }
        }
        Err(BotError::Conflict)
    }
}
