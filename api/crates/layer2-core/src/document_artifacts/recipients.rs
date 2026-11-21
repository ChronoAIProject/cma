use super::{types::valid_subject, DocumentError, DocumentService, DocumentShareRecipient};
use crate::{Caller, PublicIdentity, StoreError, SubjectId, UserDirectory};
use std::sync::Arc;

impl From<PublicIdentity> for DocumentShareRecipient {
    fn from(identity: PublicIdentity) -> Self {
        Self {
            subject: identity.subject,
            display_name: identity.display_name,
        }
    }
}

impl DocumentService {
    pub fn with_directory(mut self: Arc<Self>, directory: Arc<UserDirectory>) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("document directory is configured before sharing")
            .directory = Some(directory);
        self
    }

    pub async fn share_recipients(
        &self,
        caller: &Caller,
        id: &str,
        query: &str,
    ) -> Result<Vec<DocumentShareRecipient>, DocumentError> {
        self.owner(caller, id).await?;
        let directory = self.recipient_directory()?;
        // A known subject must be selectable even if it is outside the fuzzy
        // search's length bounds or would fall below its ranked result limit.
        if valid_recipient(caller, query) {
            let subject = SubjectId::new(query);
            if let Some(user) = directory
                .exact_user(&subject)
                .await
                .map_err(directory_error)?
            {
                let erasing = self
                    .standing
                    .authoritative(&subject)
                    .await
                    .map_err(|_| DocumentError::Unavailable)?;
                return Ok(if erasing {
                    Vec::new()
                } else {
                    vec![user.identity.into()]
                });
            }
        }
        let matches = directory
            .share_recipients(query)
            .await
            .map_err(directory_error)?;
        let mut recipients = Vec::with_capacity(matches.len());
        for person in matches {
            if !valid_recipient(caller, person.subject.as_str()) {
                continue;
            }
            if !self
                .standing
                .erasing(&person.subject)
                .await
                .map_err(|_| DocumentError::Unavailable)?
            {
                recipients.push(person.into());
            }
        }
        Ok(recipients)
    }

    pub(super) async fn require_share_recipient(
        &self,
        caller: &Caller,
        subject: &str,
    ) -> Result<(), DocumentError> {
        if !valid_recipient(caller, subject) {
            return Err(DocumentError::InvalidShareRecipient);
        }
        let subject = SubjectId::new(subject);
        if self
            .standing
            .authoritative(&subject)
            .await
            .map_err(|_| DocumentError::Unavailable)?
            || self
                .recipient_directory()?
                .exact_user(&subject)
                .await
                .map_err(directory_error)?
                .is_none()
        {
            return Err(DocumentError::ShareRecipientUnavailable);
        }
        Ok(())
    }

    fn recipient_directory(&self) -> Result<&UserDirectory, DocumentError> {
        self.directory
            .as_deref()
            .ok_or(DocumentError::RecipientDirectoryUnavailable)
    }
}

fn valid_recipient(caller: &Caller, subject: &str) -> bool {
    valid_subject(subject)
        && subject.is_ascii()
        && subject.trim() == subject
        && subject != caller.subject.as_str()
}

fn directory_error(_: StoreError) -> DocumentError {
    tracing::warn!("document share recipient lookup failed");
    DocumentError::RecipientDirectoryUnavailable
}
