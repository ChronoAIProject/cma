//! Optional export to the billing provider already used by NyxID.
#[derive(Clone)]
pub(crate) struct UsageBillingConfig {
    pub api_url: reqwest::Url,
    pub api_key: zeroize::Zeroizing<String>,
    pub plan_code: String,
}

impl UsageBillingConfig {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Option<Self>, String> {
        match lookup("CHRONO_USAGE_BILLING_ENABLED")?.as_deref() {
            None | Some("false") => return Ok(None),
            Some("true") => {}
            _ => return Err("CHRONO_USAGE_BILLING_ENABLED must be true or false".into()),
        }
        let base = super::required(lookup, "CHRONO_USAGE_LAGO_API_URL")?;
        let api_url =
            reqwest::Url::parse(&format!("{}/", base.trim_end_matches('/'))).map_err(|_| {
                "CHRONO_USAGE_LAGO_API_URL must be the complete HTTPS API base ending /api/v1"
                    .to_string()
            })?;
        if api_url.scheme() != "https"
            || api_url.host_str().is_none()
            || !api_url.username().is_empty()
            || api_url.password().is_some()
            || api_url.query().is_some()
            || api_url.fragment().is_some()
            || !api_url.path().ends_with("/api/v1/")
        {
            return Err(
                "CHRONO_USAGE_LAGO_API_URL must be the complete HTTPS API base ending /api/v1"
                    .into(),
            );
        }
        let plan_code = super::required(lookup, "CHRONO_USAGE_LAGO_PLAN_CODE")?;
        if plan_code.len() > 128
            || !plan_code
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            return Err("CHRONO_USAGE_LAGO_PLAN_CODE must be a provider plan code".into());
        }
        Ok(Some(Self {
            api_url,
            api_key: super::required(lookup, "CHRONO_USAGE_LAGO_API_KEY")?.into(),
            plan_code,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn enabled_delivery_requires_an_explicit_https_provider_target_and_secret() {
        assert!(UsageBillingConfig::from_lookup(&mut |_| Ok(None))
            .unwrap()
            .is_none());
        for base in [
            "http://billing.example/api/v1",
            "https://user@billing.example/api/v1",
            "https://billing.example/api/v1?secret=hidden",
            "https://billing.example/",
        ] {
            let result = UsageBillingConfig::from_lookup(&mut |key| {
                Ok(Some(
                    match key {
                        "CHRONO_USAGE_BILLING_ENABLED" => "true",
                        "CHRONO_USAGE_LAGO_API_URL" => base,
                        "CHRONO_USAGE_LAGO_API_KEY" => "fixture-only",
                        "CHRONO_USAGE_LAGO_PLAN_CODE" => "starter",
                        _ => unreachable!(),
                    }
                    .into(),
                ))
            });
            assert!(result.is_err());
        }
        for missing in [
            "CHRONO_USAGE_LAGO_API_URL",
            "CHRONO_USAGE_LAGO_API_KEY",
            "CHRONO_USAGE_LAGO_PLAN_CODE",
        ] {
            assert!(UsageBillingConfig::from_lookup(&mut |key| {
                if key == missing {
                    return Ok(None);
                }
                Ok(Some(
                    match key {
                        "CHRONO_USAGE_BILLING_ENABLED" => "true",
                        "CHRONO_USAGE_LAGO_API_URL" => "https://billing.example/api/api/v1",
                        "CHRONO_USAGE_LAGO_API_KEY" => "fixture-only",
                        "CHRONO_USAGE_LAGO_PLAN_CODE" => "starter",
                        _ => unreachable!(),
                    }
                    .into(),
                ))
            })
            .is_err());
        }
    }
}
