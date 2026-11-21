use super::{types::valid_subject, DocumentError, DocumentService, DocumentShareRecipient};
use crate::{Caller, StoreError, SubjectId, UserDirectory};
use std::sync::Arc;

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
        let matches = self
            .recipient_directory()?
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
                recipients.push(DocumentShareRecipient {
                    subject: person.subject,
                    display_name: person.display_name,
                });
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
            || !self
                .recipient_directory()?
                .contains_exact_subject(&subject)
                .await
                .map_err(directory_error)?
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
