//! Shared encrypted subject decoding for point and joined identity reads.
use super::*;

impl MongoAuthStore {
    pub(super) fn subject_to_doc(&self, state: SubjectAuthState) -> Result<SubjectDoc, StoreError> {
        state.validate()?;
        let id = state.subject.as_str().as_bytes();
        let (access_token, refresh_token, token_expires_at) = match state.tokens {
            Some(tokens) => (
                Some(self.cipher.seal(
                    SUBJECT_KIND,
                    id,
                    "access_token",
                    tokens.access_token.expose(),
                )?),
                tokens
                    .refresh_token
                    .as_ref()
                    .map(|token| {
                        self.cipher
                            .seal(SUBJECT_KIND, id, "refresh_token", token.expose())
                    })
                    .transpose()?,
                Some(DateTime::from_system_time(tokens.expires_at)),
            ),
            None => (None, None, None),
        };
        Ok(SubjectDoc {
            subject: state.subject.0,
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            incarnation: state.incarnation,
            revision: state.revision,
            token_generation: state.token_generation,
            refresh_lease: state.refresh_lease.map(|lease| RefreshLeaseDoc {
                holder: lease.holder,
                expires_at: DateTime::from_system_time(lease.expires_at),
            }),
            authorization: state.authorization,
            profile: state.profile,
            access_token,
            refresh_token,
            token_expires_at,
        })
    }

    pub(super) fn subject_from_doc(
        &self,
        document: SubjectDoc,
    ) -> Result<SubjectAuthState, StoreError> {
        schema::require_current(document.document_schema_version)?;
        let id = document.subject.as_bytes();
        let tokens = match (
            document.access_token,
            document.refresh_token,
            document.token_expires_at,
        ) {
            (Some(access), refresh_token, Some(expires_at)) => Some(DurableSubjectTokens {
                access_token: SecretValue::new(self.cipher.open(
                    SUBJECT_KIND,
                    id,
                    "access_token",
                    access,
                )?),
                refresh_token: refresh_token
                    .map(|refresh| self.cipher.open(SUBJECT_KIND, id, "refresh_token", refresh))
                    .transpose()?
                    .map(SecretValue::new),
                expires_at: expires_at.to_system_time(),
            }),
            (None, None, None) => None,
            _ => {
                return Err(StoreError::Malformed(
                    "subject auth token fields are incomplete".into(),
                ))
            }
        };
        let state = SubjectAuthState {
            subject: SubjectId::new(document.subject),
            incarnation: document.incarnation,
            revision: document.revision,
            token_generation: document.token_generation,
            refresh_lease: document.refresh_lease.map(|lease| SubjectRefreshLease {
                holder: lease.holder,
                expires_at: lease.expires_at.to_system_time(),
            }),
            authorization: document.authorization,
            profile: document.profile,
            tokens,
        };
        state.validate()?;
        Ok(state)
    }
}
