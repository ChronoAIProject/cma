//! Record URLs sealed into channel IDs by an earlier deployment.
//!
//! A channel ID seals the absolute URL of its record under the CMA origin the
//! issuing gateway was configured with. Moving the gateway to another CMA
//! address must not orphan IDs that owners and integrations already hold, so
//! an operator names each earlier origin and a pointer under one of them still
//! identifies the same record. Reads always use the configured origin: a
//! retained origin is matched, never contacted.
use super::*;

/// More earlier CMA addresses than any migration needs is an operator mistake.
pub(crate) const MAX_RETAINED_ORIGINS: usize = 8;

impl RecordClient {
    /// Accept record URLs that channel IDs sealed under each of `origins`.
    pub fn with_retained_origins(mut self, origins: &[String]) -> Result<Self, RecordError> {
        if origins.len() > MAX_RETAINED_ORIGINS {
            return Err(RecordError::Rejected(
                "too many retained CMA record origins".into(),
            ));
        }
        let mut retained = Vec::with_capacity(origins.len());
        for origin in origins {
            let url = Url::parse(origin).map_err(|_| {
                RecordError::Rejected("retained CMA record origin is invalid".into())
            })?;
            if !matches!(url.scheme(), "http" | "https")
                || url.host_str().is_none()
                || url.username() != ""
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
                || (url.path() != "" && url.path() != "/")
            {
                return Err(RecordError::Rejected(
                    "retained CMA record origin must be an origin".into(),
                ));
            }
            if url.origin() != self.base_url.origin()
                && !retained
                    .iter()
                    .any(|known: &Url| known.origin() == url.origin())
            {
                retained.push(url);
            }
        }
        self.retained_origins = retained.into();
        Ok(self)
    }

    /// The configured URL of the record `record_url` names, when it has the
    /// exact record form under the configured or a retained CMA origin.
    pub fn current_record_url(&self, record_url: &str) -> Result<Url, RecordError> {
        let record_path = self.record_path;
        let url = Url::parse(record_url)
            .map_err(|error| RecordError::Rejected(format!("record URL is invalid: {error}")))?;
        let id = url
            .path()
            .strip_prefix(&format!("{record_path}/"))
            .ok_or_else(|| RecordError::Rejected("invalid record path".into()))?;
        let current = self.record_url(id)?;
        let exact_under = |base: &Url| {
            base.join(&format!("{record_path}/{id}"))
                .is_ok_and(|expected| expected.as_str() == record_url)
        };
        if current.as_str() != record_url && !self.retained_origins.iter().any(exact_under) {
            return Err(RecordError::Rejected(
                "record URL is outside the configured CMA origin".into(),
            ));
        }
        Ok(current)
    }

    /// Whether a sealed pointer's `record_url` names the record `record_id`.
    pub fn names_record(&self, record_url: &str, record_id: &str) -> bool {
        match (
            self.current_record_url(record_url),
            self.record_url(record_id),
        ) {
            (Ok(named), Ok(expected)) => named == expected,
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RECORD: &str = "6f9c1f43-5a1e-4c43-9b0e-3f8f61d1c2a7";
    const OTHER: &str = "0b5f4c8e-8f6e-4d3a-a9e2-7c1d2b3a4f50";

    fn client(retained: &[&str]) -> RecordClient {
        RecordClient::new("http://cma-agent-manager.example.svc:8080", "fixture")
            .unwrap()
            .with_retained_origins(&retained.iter().map(|o| o.to_string()).collect::<Vec<_>>())
            .unwrap()
    }

    fn sealed_under(origin: &str, record: &str) -> String {
        format!("{origin}/api/v1/gateway/channel-records/{record}")
    }

    #[test]
    fn configured_origin_names_its_record() {
        let records = client(&[]);
        let url = sealed_under("http://cma-agent-manager.example.svc:8080", RECORD);
        assert!(records.names_record(&url, RECORD));
        assert!(!records.names_record(&url, OTHER));
        assert_eq!(records.current_record_url(&url).unwrap().as_str(), url);
    }

    #[test]
    fn earlier_origin_is_refused_until_retained() {
        let earlier = sealed_under("http://cma-api.earlier.svc", RECORD);
        assert!(!client(&[]).names_record(&earlier, RECORD));
        assert!(matches!(
            client(&[]).current_record_url(&earlier),
            Err(RecordError::Rejected(_))
        ));

        let records = client(&["http://cma-api.earlier.svc"]);
        assert!(records.names_record(&earlier, RECORD));
        assert!(!records.names_record(&earlier, OTHER));
        // Reads resolve to the configured origin; the earlier one is never contacted.
        assert_eq!(
            records.current_record_url(&earlier).unwrap().as_str(),
            sealed_under("http://cma-agent-manager.example.svc:8080", RECORD)
        );
    }

    #[test]
    fn retained_origin_still_requires_the_exact_record_form() {
        let records = client(&["http://cma-api.earlier.svc"]);
        for url in [
            sealed_under("http://cma-api.unlisted.svc", RECORD),
            sealed_under("https://cma-api.earlier.svc", RECORD),
            sealed_under("http://cma-api.earlier.svc:8080", RECORD),
            format!("http://cma-api.earlier.svc/api/v1/gateway/channel-records/{RECORD}?x=1"),
            format!("http://cma-api.earlier.svc/api/v1/other/{RECORD}"),
            format!(
                "http://cma-api.earlier.svc/api/v1/gateway/channel-records/{}",
                RECORD.to_uppercase()
            ),
        ] {
            assert!(!records.names_record(&url, RECORD), "{url}");
        }
    }

    #[test]
    fn retained_origins_are_bounded_and_must_be_origins() {
        let base = RecordClient::new("http://cma.example.svc", "fixture").unwrap();
        for invalid in [
            "http://cma.example.svc/path",
            "http://user@cma.example.svc",
            "ftp://cma.example.svc",
            "not a url",
        ] {
            assert!(base
                .clone()
                .with_retained_origins(&[invalid.to_string()])
                .is_err());
        }
        let many = (0..=MAX_RETAINED_ORIGINS)
            .map(|index| format!("http://cma-{index}.example.svc"))
            .collect::<Vec<_>>();
        assert!(base.with_retained_origins(&many).is_err());
    }
}
