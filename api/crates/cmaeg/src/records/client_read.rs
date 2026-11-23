use super::*;

#[derive(Clone, Copy)]
pub(crate) enum ReadBackend {
    Mongo,
    LegacyHttp,
}
impl ReadBackend {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Mongo => "mongo",
            Self::LegacyHttp => "legacy_http",
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ReadClass {
    Validation,
    MongoRead,
    ServiceAuth,
    Timeout,
    Transport,
    HttpStatus(u16),
    ResponseDecode,
}
impl ReadClass {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Validation => "validation",
            Self::MongoRead => "mongo_read",
            Self::ServiceAuth => "service_auth",
            Self::Timeout => "timeout",
            Self::Transport => "transport",
            Self::HttpStatus(_) => "http_status",
            Self::ResponseDecode => "response_decode",
        }
    }
    pub(crate) fn http_status(self) -> Option<u16> {
        match self {
            Self::HttpStatus(status) => Some(status),
            _ => None,
        }
    }
}

pub(crate) struct ReadFailure {
    pub(crate) error: RecordError,
    pub(crate) class: ReadClass,
}
impl ReadFailure {
    fn new(error: RecordError, class: ReadClass) -> Self {
        Self { error, class }
    }
    fn transport(error: reqwest::Error) -> Self {
        let class = if error.is_timeout() {
            ReadClass::Timeout
        } else {
            ReadClass::Transport
        };
        Self::new(transport(error), class)
    }
    fn decode(error: RecordError) -> Self {
        Self::new(error, ReadClass::ResponseDecode)
    }
}
impl From<RecordError> for ReadFailure {
    fn from(error: RecordError) -> Self {
        Self::new(error, ReadClass::Validation)
    }
}

impl RecordClient {
    pub(crate) fn read_backend(&self) -> ReadBackend {
        if self.database.is_some() {
            ReadBackend::Mongo
        } else {
            ReadBackend::LegacyHttp
        }
    }

    pub async fn get(&self, record_url: &str) -> Result<RecordValue, RecordError> {
        self.get_observed(record_url)
            .await
            .map_err(|failure| failure.error)
    }

    pub(crate) async fn get_observed(&self, record_url: &str) -> Result<RecordValue, ReadFailure> {
        let record_path = self.record_path;
        // A URL sealed under a retained origin reads from the configured one.
        let url = self.current_record_url(record_url)?;
        let id = url
            .path()
            .strip_prefix(&format!("{record_path}/"))
            .ok_or_else(|| RecordError::Rejected("invalid record path".into()))?;
        if url.origin() != self.base_url.origin() {
            return Err(RecordError::Rejected(
                "record URL is outside the configured CMA origin".into(),
            )
            .into());
        }
        if let Some(db) = &self.database {
            return db
                .for_kind(self.record_kind)
                .get(id)
                .await
                .map_err(|error| ReadFailure::new(error, ReadClass::MongoRead));
        }
        let response = self
            .client
            .get(url)
            .bearer_auth(self.read_bearer().await?.as_str())
            .timeout(Duration::from_millis(1_500))
            .send()
            .await
            .map_err(ReadFailure::transport)?;
        if response.status() == StatusCode::NOT_FOUND {
            return Err(ReadFailure::new(
                RecordError::NotFound,
                ReadClass::HttpStatus(404),
            ));
        }
        let response = read_success(response).await?;
        let body = read_json::<GetResponse>(response).await?;
        let blob = STANDARD.decode(body.blob).map_err(|error| {
            ReadFailure::decode(RecordError::Malformed(format!(
                "record blob is not base64: {error}"
            )))
        })?;
        if blob.is_empty() || blob.len() > 65_536 {
            return Err(ReadFailure::decode(RecordError::Malformed(
                "invalid blob size".into(),
            )));
        }
        Ok(RecordValue {
            blob,
            version: body.version,
        })
    }

    pub async fn list(
        &self,
        owner: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RecordHead>, RecordError> {
        self.list_observed(owner, after, limit)
            .await
            .map_err(|failure| failure.error)
    }

    pub(crate) async fn list_observed(
        &self,
        owner: Option<&str>,
        after: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RecordHead>, ReadFailure> {
        self.check_thread_fence_transport()?;
        self.check_media_continuation_transport()?;
        if let Some(db) = &self.database {
            return db
                .for_kind(self.record_kind)
                .list(owner, after, limit)
                .await
                .map_err(|error| ReadFailure::new(error, ReadClass::MongoRead));
        }
        if self.record_kind == super::super::RecordKind::MediaContinuation
            && !(1..=32).contains(&limit)
        {
            return Err(
                RecordError::Rejected("invalid media continuation page size".into()).into(),
            );
        }
        let url = self
            .base_url
            .join(self.record_path)
            .map_err(|error| RecordError::Rejected(error.to_string()))?;
        let mut query = Vec::<(&str, String)>::new();
        if let Some(owner) = owner {
            query.push(("owner", owner.to_owned()));
        }
        if let Some(after) = after {
            query.push(("after", after.to_owned()));
        }
        query.push(("limit", limit.to_string()));
        let response = self
            .client
            .get(url)
            .bearer_auth(self.read_bearer().await?.as_str())
            .query(&query)
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .map_err(ReadFailure::transport)?;
        let response = read_success(response).await?;
        let body = read_json::<ListResponse>(response).await?;
        Ok(body.items)
    }

    async fn read_bearer(&self) -> Result<Secret, ReadFailure> {
        self.service_token.bearer().await.map_err(|error| {
            ReadFailure::new(
                RecordError::Unavailable(error.to_string()),
                ReadClass::ServiceAuth,
            )
        })
    }
}

async fn read_success(response: reqwest::Response) -> Result<reqwest::Response, ReadFailure> {
    let status = response.status().as_u16();
    success(response)
        .await
        .map_err(|error| ReadFailure::new(error, ReadClass::HttpStatus(status)))
}

pub(super) async fn read_json<T: serde::de::DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, ReadFailure> {
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(ReadFailure::transport)? {
        if bytes.len() + chunk.len() > 256 * 1024 {
            return Err(ReadFailure::decode(RecordError::Malformed(
                "response exceeds size limit".into(),
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes)
        .map_err(|_| ReadFailure::decode(RecordError::Malformed("invalid response JSON".into())))
}
