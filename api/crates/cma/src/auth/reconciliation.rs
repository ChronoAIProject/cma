//! Recurring reconciliation of durable cookie-session authorization.
//!
//! Sign-in establishes an initial complete answer. This lane asks again on a
//! fixed cadence, independently of token refresh, and replaces that answer
//! only when the provider supplies another complete answer. Every write is a
//! revision CAS against the token generation that was actually checked.

use std::sync::Arc;
use std::time::{Duration, SystemTime};

use futures_util::{stream, StreamExt};
use layer2_core::store::StoreError;
use layer2_core::{Authorization, SubjectId};

use super::persistence::{SubjectAuthState, SubjectAuthorizationCursor, SubjectAuthorizationLease};
use super::{
    authorization_check_deadline, next_token_generation, random_token, AuthMode,
    AuthOperationBudget, AuthState, AUTHORIZATION_CHECK_ATTEMPT_BUDGET,
    AUTHORIZATION_CHECK_BATCH_SIZE, AUTHORIZATION_CHECK_INTERVAL,
};

const AUTHORIZATION_CHECK_LEASE_LIFETIME: Duration = Duration::from_secs(30);
#[cfg(not(test))]
const REQUEST_AUTHORIZATION_CHECK_TIMEOUT: Duration = Duration::from_secs(5);
#[cfg(test)]
const REQUEST_AUTHORIZATION_CHECK_TIMEOUT: Duration = Duration::from_millis(100);

/// One page whose cursor advances independently of item outcomes.
struct AuthorizationCheckPage {
    work: Vec<SubjectAuthState>,
    next: Option<SubjectAuthorizationCursor>,
    full: bool,
    first_error: Option<StoreError>,
}

#[derive(Clone)]
enum ProviderAnswer {
    Complete {
        authorization: Authorization,
        token_disposition: TokenDisposition,
    },
    Transient(String),
}

#[derive(Clone, Copy)]
enum TokenDisposition {
    Keep,
    QuarantineAccess,
    RetireDelegation,
}

enum UserinfoAnswer {
    Complete(Vec<String>),
    SubjectMismatch,
    Transient(String),
}

enum IntrospectionAnswer {
    Active(Vec<String>),
    Inactive,
    SubjectMismatch,
    Transient(String),
}

fn complete_permissions_claim(
    body: &serde_json::Value,
    source: &str,
) -> Result<Vec<String>, String> {
    let Some(values) = body.get("permissions").and_then(|value| value.as_array()) else {
        return Err(format!("{source} carried no complete permissions array"));
    };
    values
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| format!("{source} permissions were not all strings"))
        })
        .collect()
}

/// The deadline at which nonempty authority stops being servable.
///
/// Schema-v3 records can already carry a check deadline later than their token
/// expiry. Clamp again on the request path so migration cannot briefly revive
/// that authority before the next fenced write normalizes the durable value.
fn servable_authorization_deadline(state: &SubjectAuthState) -> SystemTime {
    if state.authorization.capabilities().next().is_none() {
        return state.authorization_check_after;
    }
    let Some(tokens) = state.tokens.as_ref() else {
        return SystemTime::UNIX_EPOCH;
    };
    if tokens.access_token.is_none() {
        return SystemTime::UNIX_EPOCH;
    }
    state.authorization_check_after.min(tokens.expires_at)
}

/// Result of one selected reconciliation item.
#[derive(Debug)]
pub(crate) enum AuthorizationCheckOutcome {
    /// A complete provider answer replaced the durable grants.
    Replaced,
    /// An incomplete provider answer left the last complete grants intact.
    Preserved(String),
    /// Concurrent work made the selected row no longer actionable.
    Skipped,
}

fn take_operation(
    budget: &AuthOperationBudget,
    amount: usize,
    operation: &str,
) -> Result<(), StoreError> {
    if !budget.take(amount) {
        return Err(StoreError::Unavailable(format!(
            "recurring authorization operation budget exhausted before {operation}"
        )));
    }
    Ok(())
}

impl AuthState {
    /// Process one provider batch and advance a fairness cursor for the next sweep.
    pub(crate) async fn reconcile_due_authorizations(
        self: &Arc<Self>,
        due_at: SystemTime,
        page_budget: &AuthOperationBudget,
    ) {
        let after = self.authorization_sweep_after.lock().await.clone();
        let mut page = match self
            .authorization_check_page_with_budget(due_at, after.as_ref(), page_budget)
            .await
        {
            Ok(page) => page,
            Err(error) => {
                tracing::error!(error = %error, "could not list subjects due for authorization reconciliation");
                return;
            }
        };
        if page.next.is_none() && after.is_some() {
            page = match self
                .authorization_check_page_with_budget(due_at, None, page_budget)
                .await
            {
                Ok(page) => page,
                Err(error) => {
                    tracing::error!(error = %error, "could not wrap the authorization reconciliation cursor");
                    return;
                }
            };
        }
        if let Some(error) = page.first_error {
            tracing::error!(
                error = %error,
                "one authorization reconciliation record could not be read"
            );
        }

        let next_after = if page.full { page.next.clone() } else { None };
        *self.authorization_sweep_after.lock().await = next_after;
        let outcomes = stream::iter(page.work)
            .map(|state| {
                let auth = Arc::clone(self);
                async move {
                    let subject = state.subject.clone();
                    let budget =
                        AuthOperationBudget::standalone(AUTHORIZATION_CHECK_ATTEMPT_BUDGET);
                    let outcome = auth
                        .reconcile_authorization_with_budget(state, due_at, &budget)
                        .await;
                    (subject, outcome)
                }
            })
            .buffer_unordered(AUTHORIZATION_CHECK_BATCH_SIZE)
            .collect::<Vec<_>>()
            .await;

        for (subject, outcome) in outcomes {
            match outcome {
                Ok(AuthorizationCheckOutcome::Replaced) => {
                    tracing::info!(%subject, "reconciled subject authorization");
                }
                Ok(AuthorizationCheckOutcome::Preserved(error)) => {
                    tracing::warn!(
                        %subject,
                        error = %error,
                        "provider authorization answer was incomplete; retained prior grants"
                    );
                }
                Ok(AuthorizationCheckOutcome::Skipped) => {}
                Err(error) => {
                    tracing::warn!(
                        %subject,
                        error = %error,
                        "authorization remains due after bounded reconciliation contention"
                    );
                }
            }
        }
    }

    /// Select one page. Per-item CAS budgets never reduce its row count.
    async fn authorization_check_page_with_budget(
        &self,
        now: SystemTime,
        after: Option<&SubjectAuthorizationCursor>,
        budget: &AuthOperationBudget,
    ) -> Result<AuthorizationCheckPage, StoreError> {
        take_operation(budget, 1, "authorization due-page query")?;
        let selected = self
            .store
            .subjects_due_for_authorization_check(now, after, AUTHORIZATION_CHECK_BATCH_SIZE)
            .await?;
        let selected_len = selected.len();
        if selected_len > AUTHORIZATION_CHECK_BATCH_SIZE {
            return Err(StoreError::Malformed(
                "subject authorization store exceeded its requested page limit".into(),
            ));
        }
        take_operation(
            budget,
            selected_len * AUTHORIZATION_CHECK_ATTEMPT_BUDGET,
            "selected authorization item reservations",
        )?;

        let full = selected_len == AUTHORIZATION_CHECK_BATCH_SIZE;
        let next = selected.last().map(|item| item.cursor.clone());
        let mut first_error = None;
        let mut work = Vec::with_capacity(selected_len);
        for item in selected {
            match item.state {
                Ok(state) => work.push(state),
                Err(error) => {
                    tracing::warn!(
                        error = %error,
                        subject = %item.cursor.subject,
                        "one authorization reconciliation record could not be read"
                    );
                    if first_error.is_none() {
                        first_error = Some(error);
                    }
                }
            }
        }
        budget.refund(selected_len.saturating_sub(work.len()) * AUTHORIZATION_CHECK_ATTEMPT_BUDGET);
        Ok(AuthorizationCheckPage {
            work,
            next,
            full,
            first_error,
        })
    }

    /// Reconcile one selected fact against a durable claim on the exact token checked.
    pub(crate) async fn reconcile_authorization_with_budget(
        &self,
        current: SubjectAuthState,
        due_at: SystemTime,
        budget: &AuthOperationBudget,
    ) -> Result<AuthorizationCheckOutcome, StoreError> {
        let Some(claimed) = self
            .claim_authorization_check(current, due_at, budget)
            .await?
        else {
            return Ok(AuthorizationCheckOutcome::Skipped);
        };
        let tokens = claimed
            .tokens
            .as_ref()
            .expect("an authorization claim is issued only for token state");
        let answer = if tokens.expires_at <= SystemTime::now() && tokens.refresh_token.is_none() {
            ProviderAnswer::Complete {
                authorization: Authorization::none(),
                token_disposition: TokenDisposition::RetireDelegation,
            }
        } else {
            let access_token = tokens
                .access_token
                .as_ref()
                .expect("an authorization claim is issued only for a usable access token")
                .expose()
                .to_string();
            self.resolve_authorization_answer(&claimed.subject, &access_token, budget)
                .await
        };
        self.complete_authorization_check(claimed, answer, budget)
            .await
    }

    async fn claim_authorization_check(
        &self,
        mut current: SubjectAuthState,
        due_at: SystemTime,
        budget: &AuthOperationBudget,
    ) -> Result<Option<SubjectAuthState>, StoreError> {
        loop {
            let now = SystemTime::now();
            if current.authorization_check_after > due_at || current.tokens.is_none() {
                return Ok(None);
            }
            if current
                .authorization_lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at > now)
                || current
                    .refresh_lease
                    .as_ref()
                    .is_some_and(|lease| lease.expires_at > now)
            {
                return Ok(None);
            }
            let tokens = current
                .tokens
                .as_ref()
                .expect("token presence was checked above");
            // Refresh owns expired renewable tokens. Keeping this row due makes
            // cookie requests fail closed until the refreshed generation has a
            // complete authorization answer.
            if tokens.access_token.is_none()
                || (tokens.expires_at <= now && tokens.refresh_token.is_some())
            {
                return Ok(None);
            }

            let fence = current
                .revision
                .checked_add(1)
                .ok_or_else(|| StoreError::Malformed("subject auth revision exhausted".into()))?;
            let mut next = current.clone();
            next.revision = fence;
            next.authorization_lease = Some(SubjectAuthorizationLease {
                holder: random_token(),
                fence,
                acquired_at: now,
                expires_at: now + AUTHORIZATION_CHECK_LEASE_LIFETIME,
                checked_incarnation: current.incarnation.clone(),
                checked_token_generation: current.token_generation,
            });
            take_operation(budget, 2, "authorization-check lease acquisition")?;
            if self
                .store
                .replace_subject(&current.incarnation, current.revision, next.clone())
                .await?
            {
                return Ok(Some(next));
            }

            take_operation(budget, 1, "authorization-check lease contention read")?;
            let Some(winner) = self.store.subject(&current.subject).await? else {
                return Ok(None);
            };
            current = winner;
        }
    }

    async fn complete_authorization_check(
        &self,
        mut current: SubjectAuthState,
        answer: ProviderAnswer,
        budget: &AuthOperationBudget,
    ) -> Result<AuthorizationCheckOutcome, StoreError> {
        loop {
            let Some(lease) = current.authorization_lease.as_ref() else {
                return Ok(AuthorizationCheckOutcome::Skipped);
            };
            if lease.checked_incarnation != current.incarnation
                || lease.checked_token_generation != current.token_generation
            {
                return Ok(AuthorizationCheckOutcome::Skipped);
            }

            let mut next = current.clone();
            next.revision = current
                .revision
                .checked_add(1)
                .ok_or_else(|| StoreError::Malformed("subject auth revision exhausted".into()))?;
            next.authorization_lease = None;
            if let ProviderAnswer::Complete {
                authorization,
                token_disposition,
            } = &answer
            {
                let completed_at = SystemTime::now();
                next.authorization = authorization.clone();
                next.authorization_check_after = completed_at + AUTHORIZATION_CHECK_INTERVAL;
                match token_disposition {
                    TokenDisposition::Keep => {
                        let tokens = next
                            .tokens
                            .as_ref()
                            .expect("a kept authorization answer has token custody");
                        next.authorization_check_after =
                            authorization_check_deadline(tokens, completed_at);
                    }
                    TokenDisposition::QuarantineAccess => {
                        next.token_generation = next_token_generation(current.token_generation)?;
                        if next
                            .tokens
                            .as_ref()
                            .is_some_and(|tokens| tokens.refresh_token.is_some())
                        {
                            let tokens = next
                                .tokens
                                .as_mut()
                                .expect("refresh eligibility was checked above");
                            tokens.access_token = None;
                            tokens.expires_at = completed_at;
                            next.refresh_lease = None;
                        } else {
                            next.tokens = None;
                            next.refresh_lease = None;
                        }
                    }
                    TokenDisposition::RetireDelegation => {
                        next.token_generation = next_token_generation(current.token_generation)?;
                        next.tokens = None;
                        next.refresh_lease = None;
                    }
                }
            }

            take_operation(budget, 2, "authorization-check completion")?;
            if self
                .store
                .replace_subject(&current.incarnation, current.revision, next)
                .await?
            {
                return Ok(match answer {
                    ProviderAnswer::Complete { .. } => AuthorizationCheckOutcome::Replaced,
                    ProviderAnswer::Transient(error) => AuthorizationCheckOutcome::Preserved(error),
                });
            }

            take_operation(budget, 1, "authorization-check completion contention read")?;
            let Some(winner) = self.store.subject(&current.subject).await? else {
                return Ok(AuthorizationCheckOutcome::Skipped);
            };
            let same_claim = winner
                .authorization_lease
                .as_ref()
                .is_some_and(|winner_lease| {
                    winner_lease.holder == lease.holder
                        && winner_lease.fence == lease.fence
                        && winner_lease.checked_incarnation == lease.checked_incarnation
                        && winner_lease.checked_token_generation == lease.checked_token_generation
                });
            if !same_claim {
                return Ok(AuthorizationCheckOutcome::Skipped);
            }
            current = winner;
        }
    }

    async fn resolve_authorization_answer(
        &self,
        expected_subject: &SubjectId,
        access_token: &str,
        budget: &AuthOperationBudget,
    ) -> ProviderAnswer {
        if let Err(error) = take_operation(budget, 1, "provider metadata read") {
            return ProviderAnswer::Transient(error.to_string());
        }
        let meta = match self.metadata().await {
            Ok(meta) => meta,
            Err(error) => return ProviderAnswer::Transient(error),
        };

        match (
            meta.introspection_endpoint.as_deref(),
            meta.userinfo_endpoint.as_deref(),
        ) {
            (Some(introspection), Some(userinfo)) => {
                if let Err(error) = take_operation(budget, 1, "provider introspection check") {
                    return ProviderAnswer::Transient(error.to_string());
                }
                let introspection = self
                    .introspection_answer(introspection, expected_subject, access_token)
                    .await;
                if !matches!(&introspection, IntrospectionAnswer::Transient(_)) {
                    return Self::answer_from_introspection(introspection);
                }
                if let Err(error) = take_operation(budget, 1, "provider userinfo fallback") {
                    return ProviderAnswer::Transient(error.to_string());
                }
                let userinfo = self
                    .userinfo_answer(userinfo, expected_subject, access_token)
                    .await;
                Self::combine_provider_answers(introspection, userinfo)
            }
            (Some(introspection), None) => {
                if let Err(error) = take_operation(budget, 1, "provider introspection check") {
                    return ProviderAnswer::Transient(error.to_string());
                }
                Self::answer_from_introspection(
                    self.introspection_answer(introspection, expected_subject, access_token)
                        .await,
                )
            }
            (None, Some(userinfo)) => {
                if let Err(error) = take_operation(budget, 1, "provider userinfo check") {
                    return ProviderAnswer::Transient(error.to_string());
                }
                Self::answer_from_userinfo(
                    self.userinfo_answer(userinfo, expected_subject, access_token)
                        .await,
                )
            }
            (None, None) => ProviderAnswer::Transient(
                "provider advertises neither introspection nor userinfo".into(),
            ),
        }
    }

    async fn introspection_answer(
        &self,
        endpoint: &str,
        expected_subject: &SubjectId,
        access_token: &str,
    ) -> IntrospectionAnswer {
        let AuthMode::Provider {
            client_id,
            client_secret,
            ..
        } = &self.mode
        else {
            return IntrospectionAnswer::Transient("no provider configured".into());
        };
        let form = [
            ("token", access_token),
            ("token_type_hint", "access_token"),
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
        ];
        let response = match self.http.post(endpoint).form(&form).send().await {
            Ok(response) => response,
            Err(error) => {
                return IntrospectionAnswer::Transient(format!(
                    "introspection request failed: {error}"
                ));
            }
        };
        if !response.status().is_success() {
            return IntrospectionAnswer::Transient(format!(
                "introspection returned {}",
                response.status()
            ));
        }
        let body: serde_json::Value = match response.json().await {
            Ok(body) => body,
            Err(error) => {
                return IntrospectionAnswer::Transient(format!(
                    "introspection body not usable: {error}"
                ));
            }
        };
        let Some(active) = body.get("active").and_then(|value| value.as_bool()) else {
            return IntrospectionAnswer::Transient(
                "introspection body carried no active boolean".into(),
            );
        };
        if !active {
            return IntrospectionAnswer::Inactive;
        }
        match body.get("sub") {
            None => {}
            Some(serde_json::Value::String(subject)) if subject.is_empty() => {
                return IntrospectionAnswer::Transient(
                    "introspection body carried an empty subject claim".into(),
                );
            }
            Some(serde_json::Value::String(subject)) if subject != expected_subject.as_str() => {
                return IntrospectionAnswer::SubjectMismatch;
            }
            Some(serde_json::Value::String(_)) => {}
            Some(_) => {
                return IntrospectionAnswer::Transient(
                    "introspection body subject claim was not a string".into(),
                );
            }
        }
        match complete_permissions_claim(&body, "introspection body") {
            Ok(permissions) => IntrospectionAnswer::Active(permissions),
            Err(error) => IntrospectionAnswer::Transient(error),
        }
    }

    async fn userinfo_answer(
        &self,
        endpoint: &str,
        expected_subject: &SubjectId,
        access_token: &str,
    ) -> UserinfoAnswer {
        let response = match self
            .http
            .get(endpoint)
            .bearer_auth(access_token)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                return UserinfoAnswer::Transient(format!("userinfo request failed: {error}"));
            }
        };
        if !response.status().is_success() {
            return UserinfoAnswer::Transient(format!("userinfo returned {}", response.status()));
        }
        let body: serde_json::Value = match response.json().await {
            Ok(body) => body,
            Err(error) => {
                return UserinfoAnswer::Transient(format!("userinfo body not usable: {error}"));
            }
        };
        let Some(subject) = body
            .get("sub")
            .and_then(|value| value.as_str())
            .filter(|subject| !subject.is_empty())
        else {
            return UserinfoAnswer::Transient("userinfo carried no subject claim".into());
        };
        if subject != expected_subject.as_str() {
            return UserinfoAnswer::SubjectMismatch;
        }
        match complete_permissions_claim(&body, "userinfo body") {
            Ok(permissions) => UserinfoAnswer::Complete(permissions),
            Err(error) => UserinfoAnswer::Transient(error),
        }
    }

    fn complete_permissions(permissions: &[String]) -> ProviderAnswer {
        ProviderAnswer::Complete {
            authorization: crate::permissions::capabilities_from_permissions(permissions),
            token_disposition: TokenDisposition::Keep,
        }
    }

    fn answer_from_introspection(answer: IntrospectionAnswer) -> ProviderAnswer {
        match answer {
            IntrospectionAnswer::Active(permissions) => Self::complete_permissions(&permissions),
            IntrospectionAnswer::Inactive => ProviderAnswer::Complete {
                authorization: Authorization::none(),
                token_disposition: TokenDisposition::QuarantineAccess,
            },
            IntrospectionAnswer::SubjectMismatch => ProviderAnswer::Complete {
                authorization: Authorization::none(),
                token_disposition: TokenDisposition::RetireDelegation,
            },
            IntrospectionAnswer::Transient(error) => ProviderAnswer::Transient(error),
        }
    }

    fn answer_from_userinfo(answer: UserinfoAnswer) -> ProviderAnswer {
        match answer {
            UserinfoAnswer::Complete(permissions) => Self::complete_permissions(&permissions),
            UserinfoAnswer::SubjectMismatch => ProviderAnswer::Complete {
                authorization: Authorization::none(),
                token_disposition: TokenDisposition::RetireDelegation,
            },
            UserinfoAnswer::Transient(error) => ProviderAnswer::Transient(error),
        }
    }

    fn combine_provider_answers(
        introspection: IntrospectionAnswer,
        userinfo: UserinfoAnswer,
    ) -> ProviderAnswer {
        if matches!(&introspection, IntrospectionAnswer::SubjectMismatch)
            || matches!(&userinfo, UserinfoAnswer::SubjectMismatch)
        {
            return ProviderAnswer::Complete {
                authorization: Authorization::none(),
                token_disposition: TokenDisposition::RetireDelegation,
            };
        }
        match introspection {
            IntrospectionAnswer::Active(permissions) => Self::complete_permissions(&permissions),
            IntrospectionAnswer::Inactive => ProviderAnswer::Complete {
                authorization: Authorization::none(),
                token_disposition: TokenDisposition::QuarantineAccess,
            },
            IntrospectionAnswer::SubjectMismatch => unreachable!("handled above"),
            IntrospectionAnswer::Transient(introspection_error) => match userinfo {
                UserinfoAnswer::Complete(permissions) => Self::complete_permissions(&permissions),
                UserinfoAnswer::SubjectMismatch => unreachable!("handled above"),
                UserinfoAnswer::Transient(userinfo_error) => {
                    ProviderAnswer::Transient(format!("{introspection_error}; {userinfo_error}"))
                }
            },
        }
    }

    /// Require a fresh complete answer before an overdue cookie can carry authority.
    pub(super) async fn enforce_cookie_authorization(
        &self,
        mut current: SubjectAuthState,
    ) -> Result<SubjectAuthState, StoreError> {
        let due_at = SystemTime::now();
        let servable_until = servable_authorization_deadline(&current);
        if servable_until > due_at {
            return Ok(current);
        }
        current.authorization_check_after = servable_until;
        let subject = current.subject.clone();
        let budget = AuthOperationBudget::standalone(AUTHORIZATION_CHECK_ATTEMPT_BUDGET);
        tokio::time::timeout(
            REQUEST_AUTHORIZATION_CHECK_TIMEOUT,
            self.reconcile_authorization_with_budget(current, due_at, &budget),
        )
        .await
        .map_err(|_| {
            StoreError::Unavailable(
                "provider authorization check exceeded the request-time allowance".into(),
            )
        })??;

        let latest = self.store.subject(&subject).await?.ok_or_else(|| {
            StoreError::Unavailable("subject authorization disappeared during its check".into())
        })?;
        if servable_authorization_deadline(&latest) <= SystemTime::now() {
            return Err(StoreError::Unavailable(
                "subject authorization is overdue or another worker is checking it".into(),
            ));
        }
        Ok(latest)
    }
}
