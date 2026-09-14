use std::time::{Duration, SystemTime};

use layer2_core::{Authorization, StoreError, SubjectId};

use crate::auth::persistence::{
    next_auth_counter, DurableSubjectTokens, SubjectAuthState, SubjectRefreshLease,
};
use crate::auth::{token, AuthState};

const LEASE_LIFETIME: Duration = Duration::from_secs(45);
const WAIT_LIMIT: Duration = Duration::from_secs(20);
const POLL: Duration = Duration::from_millis(50);

pub(super) struct Claim {
    subject: SubjectId,
    incarnation: String,
    generation: u64,
    holder: String,
}

pub(super) enum Acquisition {
    Owned(Claim, SubjectAuthState),
    Current(SubjectAuthState),
}

pub(super) async fn read(
    auth: &AuthState,
    subject: &SubjectId,
) -> Result<SubjectAuthState, StoreError> {
    let state = auth.store.subject(subject).await?.ok_or_else(|| {
        StoreError::Unavailable("subject authority disappeared during refresh".into())
    })?;
    state.validate()?;
    if state.subject != *subject {
        return Err(StoreError::Malformed(
            "subject authority crossed identity".into(),
        ));
    }
    Ok(state)
}

pub(super) async fn acquire(
    auth: &AuthState,
    observed: &SubjectAuthState,
) -> Result<Acquisition, StoreError> {
    observed.validate()?;
    tokio::time::timeout(WAIT_LIMIT, async {
        loop {
            let current = read(auth, &observed.subject).await?;
            if changed(observed, &current) || !super::due(&current) {
                return Ok(Acquisition::Current(current));
            }
            if current.tokens != observed.tokens {
                return Err(StoreError::Malformed(
                    "subject tokens changed without a generation".into(),
                ));
            }
            if current
                .refresh_lease
                .as_ref()
                .is_some_and(|lease| lease.expires_at > SystemTime::now())
            {
                tokio::time::sleep(POLL).await;
                continue;
            }
            let holder = token::random_token();
            let mut next = current.clone();
            next.revision = next_auth_counter(current.revision)?;
            next.refresh_lease = Some(SubjectRefreshLease {
                holder: holder.clone(),
                expires_at: SystemTime::now() + LEASE_LIFETIME,
            });
            if auth
                .store
                .replace_subject(&current.incarnation, current.revision, next.clone())
                .await?
            {
                return Ok(Acquisition::Owned(
                    Claim {
                        subject: current.subject,
                        incarnation: current.incarnation,
                        generation: current.token_generation,
                        holder,
                    },
                    next,
                ));
            }
            tokio::time::sleep(POLL).await;
        }
    })
    .await
    .map_err(|_| StoreError::Unavailable("subject refresh is already leased".into()))?
}

pub(super) fn changed(observed: &SubjectAuthState, current: &SubjectAuthState) -> bool {
    observed.incarnation != current.incarnation
        || observed.token_generation != current.token_generation
}

impl Claim {
    fn owns(&self, current: &SubjectAuthState) -> bool {
        current.incarnation == self.incarnation
            && current.token_generation == self.generation
            && current
                .refresh_lease
                .as_ref()
                .is_some_and(|lease| lease.holder == self.holder)
    }

    pub(super) async fn finish(
        &self,
        auth: &AuthState,
        tokens: Option<DurableSubjectTokens>,
    ) -> Result<Option<SubjectAuthState>, StoreError> {
        for _ in 0..32 {
            let current = read(auth, &self.subject).await?;
            if !self.owns(&current)
                || current
                    .refresh_lease
                    .as_ref()
                    .is_none_or(|lease| lease.expires_at <= SystemTime::now())
            {
                return Ok(None);
            }
            // Preserve grants and display state from the latest revision, never
            // the snapshot that was sent to the provider.
            let mut next = current.clone();
            next.revision = next_auth_counter(current.revision)?;
            next.token_generation = next_auth_counter(current.token_generation)?;
            next.refresh_lease = None;
            next.tokens = tokens.clone();
            if next.tokens.is_none() {
                next.authorization = Authorization::none();
            }
            if auth
                .store
                .replace_subject(&current.incarnation, current.revision, next.clone())
                .await?
            {
                return Ok(Some(next));
            }
        }
        Err(StoreError::Unavailable(
            "subject refresh completion remained contended".into(),
        ))
    }

    pub(super) async fn abandon(&self, auth: &AuthState) -> Result<(), StoreError> {
        for _ in 0..32 {
            let current = read(auth, &self.subject).await?;
            if !self.owns(&current) {
                return Ok(());
            }
            let mut next = current.clone();
            next.revision = next_auth_counter(current.revision)?;
            next.refresh_lease = None;
            if auth
                .store
                .replace_subject(&current.incarnation, current.revision, next)
                .await?
            {
                return Ok(());
            }
        }
        Err(StoreError::Unavailable(
            "subject refresh release remained contended".into(),
        ))
    }
}
