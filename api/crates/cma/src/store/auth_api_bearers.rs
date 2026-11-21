//! Immutable API bearer custody; it never changes browser-login or refresh state.

use super::*;

const KIND: &str = "api-bearer-custody";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ApiBearerDoc {
    #[serde(rename = "_id")]
    pub(super) id: Binary,
    pub(super) document_schema_version: i64,
    pub(super) issuer: String,
    pub(super) subject: String,
    pub(super) credential_id: String,
    pub(super) access_token: CipherEnvelope,
    pub(super) expires_at: Option<DateTime>,
}

fn binding(
    id: &Binary,
    issuer: &str,
    subject: &str,
    credential_id: &str,
    expires_at: Option<DateTime>,
) -> Result<Vec<u8>, StoreError> {
    serde_json::to_vec(&(
        &id.bytes,
        issuer,
        subject,
        credential_id,
        expires_at.map(|value| value.timestamp_millis()),
    ))
    .map_err(|_| StoreError::Malformed("API bearer custody binding is invalid".into()))
}

impl MongoAuthStore {
    pub(super) async fn ensure_bearer_subject_index(&self) -> Result<(), StoreError> {
        self.api_bearers
            .create_index(
                IndexModel::builder()
                    .keys(doc! {"subject":1,"_id":1})
                    .options(
                        IndexOptions::builder()
                            .name("auth_api_bearers_subject".to_owned())
                            .build(),
                    )
                    .build(),
            )
            .await
            .map_err(|_| StoreError::Unavailable("API bearer subject index".into()))?;
        Ok(())
    }

    pub(super) fn api_bearer_to_doc(
        &self,
        custody: ApiBearerCustody,
    ) -> Result<ApiBearerDoc, StoreError> {
        custody.validate()?;
        let id = binary(*custody.id.as_bytes());
        let expires_at = custody.expires_at.map(DateTime::from_system_time);
        let aad = binding(
            &id,
            &custody.issuer,
            custody.subject.as_str(),
            &custody.credential_id,
            expires_at,
        )?;
        Ok(ApiBearerDoc {
            id,
            document_schema_version: DOCUMENT_SCHEMA_VERSION,
            issuer: custody.issuer,
            subject: custody.subject.0,
            credential_id: custody.credential_id,
            access_token: self.cipher.seal(
                KIND,
                &aad,
                "access_token",
                custody.access_token.expose(),
            )?,
            expires_at,
        })
    }

    pub(super) fn api_bearer_from_doc(
        &self,
        document: ApiBearerDoc,
    ) -> Result<ApiBearerCustody, StoreError> {
        schema::require_current(document.document_schema_version)?;
        let id = digest(&document.id, "API bearer custody")?;
        let aad = binding(
            &document.id,
            &document.issuer,
            &document.subject,
            &document.credential_id,
            document.expires_at,
        )?;
        let custody = ApiBearerCustody {
            id,
            issuer: document.issuer,
            subject: SubjectId::new(document.subject),
            credential_id: document.credential_id,
            access_token: SecretValue::new(self.cipher.open(
                KIND,
                &aad,
                "access_token",
                document.access_token,
            )?),
            expires_at: document.expires_at.map(|value| value.to_system_time()),
        };
        custody.validate()?;
        Ok(custody)
    }

    pub(super) async fn insert_api_bearer_record(
        &self,
        custody: ApiBearerCustody,
    ) -> Result<bool, StoreError> {
        let document = self.api_bearer_to_doc(custody)?;
        match self.api_bearers.insert_one(document).await {
            Ok(_) => Ok(true),
            Err(error) if super::super::sandbox_mongo::is_duplicate_key(&error) => Ok(false),
            Err(_) => Err(StoreError::Unavailable("insert API bearer custody".into())),
        }
    }

    pub(super) async fn read_api_bearer_record(
        &self,
        id: &SecretDigest,
    ) -> Result<Option<ApiBearerCustody>, StoreError> {
        self.api_bearers
            .clone_with_type::<mongodb::bson::Document>()
            .find_one(doc! { "_id": binary(*id.as_bytes()) })
            .await
            .map_err(|_| StoreError::Unavailable("read API bearer custody".into()))?
            .map(|document| {
                let document =
                    mongodb::bson::deserialize_from_document(document).map_err(|_| {
                        StoreError::Malformed("API bearer custody document is invalid".into())
                    })?;
                self.api_bearer_from_doc(document)
            })
            .transpose()
    }
}
