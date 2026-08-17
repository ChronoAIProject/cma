//! Handing a session's runtime a credential, and taking it back.
//!
//! Split from the state so that file is about who is signed in and this one
//! about what an agent is allowed to spend on their behalf. They are different
//! questions with different lifetimes: a person stays signed in across many
//! sessions, and a credential is delegated to one runtime and reclaimed when
//! that runtime stops.

use layer2_core::{SessionId, SubjectId};

use super::{AuthState, Issued};

/// Supply the core with an owner's credential when the owner is absent.
///
/// The core recovers sessions on its own — after a restart, or when a runtime
/// has to be replaced — and a recovered session that presents no credential is
/// a session whose every model call fails. So the credential is fetched from
/// wherever it is currently held rather than being carried along from the
/// request that happened to create the session.
#[async_trait::async_trait]
impl layer2_core::CredentialSource for AuthState {
    async fn credential_for(
        &self,
        session: &SessionId,
        subject: &SubjectId,
        services: &[layer2_core::ServiceId],
    ) -> Option<String> {
        let issuer = { self.issuer.read().await.clone() };
        let Some(issuer) = issuer else {
            // No platform configured. The session gets its owner's own token,
            // which is what it got before any of this existed.
            return self.access_token(subject).await;
        };

        // One value per session. Asking again — for a replaced runtime, or
        // because a restart lost the entry — must not leave two credentials
        // alive for one session.
        if let Some(held) = self.issued.read().await.get(session) {
            return Some(held.value.clone());
        }

        let owner_token = self.access_token(subject).await?;
        let minted = issuer
            .mint(session, subject, &owner_token, services)
            .await?;
        self.issued.write().await.insert(
            session.clone(),
            Issued {
                id: minted.id,
                value: minted.full_key.clone(),
                owner: subject.clone(),
                minted_at: std::time::Instant::now(),
            },
        );
        Some(minted.full_key)
    }

    async fn release(&self, session: &SessionId) {
        let Some(held) = self.issued.write().await.remove(session) else {
            return;
        };
        let Some(issuer) = ({ self.issuer.read().await.clone() }) else {
            return;
        };
        let Some(owner_token) = self.access_token(&held.owner).await else {
            // Reclaiming needs the owner's own token, which this process may
            // not hold. The key expires on its own, so this costs a row in a
            // list rather than a credential left open.
            tracing::debug!(session_id = %session, "no owner token to reclaim this session's credential with");
            return;
        };
        issuer.revoke(&held.id, &owner_token).await;
    }
}
