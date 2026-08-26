//! Recurring reconciliation of durable cookie-session authorization.
//!
//! Sign-in establishes an initial complete answer. This lane asks again on a
//! fixed cadence, independently of token refresh, and replaces that answer
//! only when the provider supplies another complete answer. Every write is a
//! revision CAS against the token generation that was actually checked.

use std::sync::Arc;
use std::time::SystemTime;

use futures_util::{stream, StreamExt};
use layer2_core::store::StoreError;
use layer2_core::{Authorization, SubjectId};

use super::persistence::{SubjectAuthState, SubjectAuthorizationCursor};
use super::{
    next_token_generation, AuthMode, AuthOperationBudget, AuthState,
    AUTHORIZATION_CHECK_ATTEMPT_BUDGET, AUTHORIZATION_CHECK_PAGE_BUDGET,
    AUTH_SWEEP_BATCH_SIZE,
};

const AUTHORIZATION_CHECK_CONCURRENCY: usize = 16;

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
        retire_tokens: bool,
    },
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
    /// Drain every due page admitted by the explicit page-query allowance.
    pub(crate) async fn reconcile_due_authorizations(
        self: &Arc<Self>,
        due_at: SystemTime,
        next_check: SystemTime,
        page_budget: &AuthOperationBudget,
    ) {
        let mut after = None;
        let mut work_pages_remaining = AUTHORIZATION_CHECK_PAGE_BUDGET - 1;
        loop {
            if work_pages_remaining == 0 {
                if let Err(error) = take_operation(
                    page_budget,
                    1,
                    "authorization due-backlog probe",
                ) {
                    tracing::error!(error = %error, "could not reserve the authorization backlog probe");
                    break;
                }
                let probe = self
                    .store
                    .subjects_due_for_authorization_check(due_at, after.as_ref(), 1)
                    .await;
                match probe {
                    Ok(items) if items.len() <= 1 => {
                        if !items.is_empty() {
                            tracing::warn!(
                                subjects = (AUTHORIZATION_CHECK_PAGE_BUDGET - 1)
                                    * AUTH_SWEEP_BATCH_SIZE,
                                "authorization reconciliation reached its per-sweep subject ceiling"
                            );
                        }
                    }
                    Ok(_) => tracing::error!(
                        "subject authorization store exceeded its backlog-probe limit"
                    ),
                    Err(error) => tracing::error!(
                        error = %error,
                        "could not probe the remaining authorization reconciliation backlog"
                    ),
                }
                break;
            }
            let page = match self
                .authorization_check_page_with_budget(due_at, after.as_ref(), page_budget)
                .await
            {
                Ok(page) => page,
                Err(error) => {
                    tracing::error!(error = %error, "could not list subjects due for authorization reconciliation");
                    break;
                }
            };
            work_pages_remaining -= 1;
            if let Some(error) = page.first_error {
                tracing::error!(
                    error = %error,
                    "one authorization reconciliation record could not be read"
                );
            }

            let full = page.full;
            let next = page.next;
            let outcomes = stream::iter(page.work)
                .map(|state| {
                    let auth = Arc::clone(self);
                    async move {
                        let subject = state.subject.clone();
                        let budget = AuthOperationBudget::standalone(
                            AUTHORIZATION_CHECK_ATTEMPT_BUDGET,
                        );
                        let outcome = auth
                            .reconcile_authorization_with_budget(
                                state,
                                due_at,
                                next_check,
                                &budget,
                            )
                            .await;
                        (subject, outcome)
                    }
                })
                .buffer_unordered(AUTHORIZATION_CHECK_CONCURRENCY)
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

            if !full {
                break;
            }
            let Some(next) = next else {
                tracing::error!("full authorization page carried no continuation cursor");
                break;
            };
            after = Some(next);
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
            .subjects_due_for_authorization_check(now, after, AUTH_SWEEP_BATCH_SIZE)
            .await?;
        let selected_len = selected.len();
        if selected_len > AUTH_SWEEP_BATCH_SIZE {
            return Err(StoreError::Malformed(
                "subject authorization store exceeded its requested page limit".into(),
            ));
        }
        take_operation(
            budget,
            selected_len * AUTHORIZATION_CHECK_ATTEMPT_BUDGET,
            "selected authorization item reservations",
        )?;

        let full = selected_len == AUTH_SWEEP_BATCH_SIZE;
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
        budget.refund(
            selected_len.saturating_sub(work.len()) * AUTHORIZATION_CHECK_ATTEMPT_BUDGET,
        );
        Ok(AuthorizationCheckPage {
            work,
            next,
            full,
            first_error,
        })
    }

    /// Reconcile one selected fact against the exact lifecycle and token generation checked.
    pub(crate) async fn reconcile_authorization_with_budget(
        &self,
        mut current: SubjectAuthState,
        due_at: SystemTime,
        next_check: SystemTime,
        budget: &AuthOperationBudget,
    ) -> Result<AuthorizationCheckOutcome, StoreError> {
        let mut cached: Option<(String, u64, ProviderAnswer)> = None;

        loop {
            if current.authorization_check_after > due_at || current.tokens.is_none() {
                return Ok(AuthorizationCheckOutcome::Skipped);
            }
            // A refresh worker owns token mutation while this lease is live.
            // Leaving the deadline untouched makes this row due again after
            // the short lease rather than competing with its completion.
            if current
                .refresh_lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at > due_at)
            {
                return Ok(AuthorizationCheckOutcome::Skipped);
            }

            let tokens = current
                .tokens
                .as_ref()
                .expect("token presence was checked above");
            let answer = if tokens.expires_at <= due_at && tokens.refresh_token.is_none() {
                ProviderAnswer::Complete {
                    authorization: Authorization::none(),
                    retire_tokens: true,
                }
            } else if let Some(answer) = cached.as_ref().and_then(
                |(incarnation, generation, answer)| {
                    if incarnation == &current.incarnation
                        && *generation == current.token_generation
                    {
                        Some(answer.clone())
                    } else {
                        None
                    }
                },
            ) {
                answer
            } else {
                let resolved = self
                    .resolve_authorization_answer(
                        &current.subject,
                        tokens.access_token.expose(),
                        budget,
                    )
                    .await;
                cached = Some((
                    current.incarnation.clone(),
                    current.token_generation,
                    resolved.clone(),
                ));
                resolved
            };

            let mut next = current.clone();
            next.revision = current
                .revision
                .checked_add(1)
                .ok_or_else(|| StoreError::Malformed("subject auth revision exhausted".into()))?;
            next.authorization_check_after = next_check;
            match &answer {
                ProviderAnswer::Complete {
                    authorization,
                    retire_tokens,
                } => {
                    next.authorization = authorization.clone();
                    if *retire_tokens {
                        next.token_generation = next_token_generation(current.token_generation)?;
                        next.tokens = None;
                        next.refresh_lease = None;
                    }
                }
                ProviderAnswer::Transient(_) => {}
            }

            take_operation(budget, 2, "authorization reconciliation write")?;
            if self
                .store
                .replace_subject(&current.incarnation, current.revision, next)
                .await?
            {
                return Ok(match answer {
                    ProviderAnswer::Complete { .. } => AuthorizationCheckOutcome::Replaced,
                    ProviderAnswer::Transient(error) => {
                        AuthorizationCheckOutcome::Preserved(error)
                    }
                });
            }

            take_operation(budget, 1, "authorization reconciliation contention read")?;
            let Some(winner) = self.store.subject(&current.subject).await? else {
                return Ok(AuthorizationCheckOutcome::Skipped);
            };
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
        let Some(userinfo_endpoint) = meta.userinfo_endpoint.as_ref() else {
            return ProviderAnswer::Transient("provider advertises no userinfo endpoint".into());
        };

        if let Err(error) = take_operation(budget, 1, "provider userinfo check") {
            return ProviderAnswer::Transient(error.to_string());
        }
        let response = match self
            .http
            .get(userinfo_endpoint)
            .bearer_auth(access_token)
            .send()
            .await
        {
            Ok(response) => response,
            Err(error) => {
                return ProviderAnswer::Transient(format!("userinfo request failed: {error}"));
            }
        };
        if !response.status().is_success() {
            return ProviderAnswer::Transient(format!(
                "userinfo returned {}",
                response.status()
            ));
        }
        let body: serde_json::Value = match response.json().await {
            Ok(body) => body,
            Err(error) => {
                return ProviderAnswer::Transient(format!(
                    "userinfo body not usable: {error}"
                ));
            }
        };
        let Some(subject) = body
            .get("sub")
            .and_then(|value| value.as_str())
            .filter(|subject| !subject.is_empty())
        else {
            return ProviderAnswer::Transient("userinfo carried no subject claim".into());
        };
        if subject != expected_subject.as_str() {
            return ProviderAnswer::Complete {
                authorization: Authorization::none(),
                retire_tokens: true,
            };
        }

        let permissions = if let Some(endpoint) = meta.introspection_endpoint.as_ref() {
            let AuthMode::Provider {
                client_id,
                client_secret,
                ..
            } = &self.mode
            else {
                return ProviderAnswer::Transient("no provider configured".into());
            };
            if let Err(error) = take_operation(budget, 1, "provider introspection check") {
                return ProviderAnswer::Transient(error.to_string());
            }
            let form = [
                ("token", access_token),
                ("token_type_hint", "access_token"),
                ("client_id", client_id.as_str()),
                ("client_secret", client_secret.as_str()),
            ];
            let response = match self.http.post(endpoint).form(&form).send().await {
                Ok(response) => response,
                Err(error) => {
                    return ProviderAnswer::Transient(format!(
                        "introspection request failed: {error}"
                    ));
                }
            };
            if !response.status().is_success() {
                return ProviderAnswer::Transient(format!(
                    "introspection returned {}",
                    response.status()
                ));
            }
            let answer: serde_json::Value = match response.json().await {
                Ok(answer) => answer,
                Err(error) => {
                    return ProviderAnswer::Transient(format!(
                        "introspection body not usable: {error}"
                    ));
                }
            };
            let Some(active) = answer.get("active").and_then(|value| value.as_bool()) else {
                return ProviderAnswer::Transient(
                    "introspection body carried no active boolean".into(),
                );
            };
            if !active {
                return ProviderAnswer::Complete {
                    authorization: Authorization::none(),
                    retire_tokens: false,
                };
            }
            match complete_permissions_claim(&answer, "introspection body") {
                Ok(permissions) => permissions,
                Err(error) => return ProviderAnswer::Transient(error),
            }
        } else {
            match complete_permissions_claim(&body, "userinfo body") {
                Ok(permissions) => permissions,
                Err(error) => return ProviderAnswer::Transient(error),
            }
        };

        ProviderAnswer::Complete {
            authorization: crate::permissions::capabilities_from_permissions(&permissions),
            retire_tokens: false,
        }
    }
}
