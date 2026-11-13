//! Explicit, default-off reporting activation; source endpoints are server-owned.
#[derive(Clone, Debug)]
pub(crate) struct MetricsConfig {
    pub enabled: bool,
    pub cluster: String,
    pub source_url: Option<String>,
    pub budgets: MetricsBudgets,
    pub reporting_pool_size: u32,
}
impl MetricsConfig {
    pub(super) fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        let enabled = match lookup("CHRONO_USAGE_METRICS_ENABLED")?.as_deref() {
            None | Some("false") => false,
            Some("true") => true,
            _ => return Err("CHRONO_USAGE_METRICS_ENABLED must be true or false".into()),
        };
        let cluster = lookup("CHRONO_USAGE_METRICS_CLUSTER")?
            .unwrap_or_else(|| "example-cluster-01".into());
        if !layer3_adapter::sandbox_metrics::label(&cluster) {
            return Err("invalid usage metrics cluster label".into());
        }
        let source_url = lookup("CHRONO_USAGE_METRICS_URL")?.filter(|v| !v.is_empty());
        if let Some(url) = &source_url {
            layer3_adapter::sandbox_metrics::MetricsClient::internal(url)
                .map_err(|e| e.to_string())?;
        }
        let budgets = MetricsBudgets::from_lookup(lookup)?;
        let reporting_pool_size = reporting_pool_size(lookup)?;
        Ok(Self {
            enabled,
            cluster,
            source_url,
            budgets,
            reporting_pool_size,
        })
    }
}

pub(crate) const DEFAULT_REPORTING_POOL_SIZE: u32 = 4;

fn reporting_pool_size(
    lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
) -> Result<u32, String> {
    let Some(value) = lookup("CHRONO_USAGE_REPORTING_MONGO_MAX_POOL_SIZE")? else {
        return Ok(DEFAULT_REPORTING_POOL_SIZE);
    };
    let invalid =
        || "CHRONO_USAGE_REPORTING_MONGO_MAX_POOL_SIZE must be an integer in 1..=16".to_owned();
    if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(invalid());
    }
    let size = value.parse::<u32>().map_err(|_| invalid())?;
    if !(1..=16).contains(&size) {
        return Err(invalid());
    }
    Ok(size)
}

/// Configurable reductions preserve the cluster-wide hard ceilings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct MetricsBudgets {
    pub workers: u8,
    pub queue: u64,
    pub source_calls: u8,
    pub source_seconds: u64,
    pub report_seconds: u64,
    pub report_bytes: i64,
    pub tenant_bytes: i64,
    pub global_bytes: i64,
}

impl Default for MetricsBudgets {
    fn default() -> Self {
        Self {
            workers: 2,
            queue: 8,
            source_calls: 2,
            source_seconds: 5,
            report_seconds: 60,
            report_bytes: 256 * 1024 * 1024,
            tenant_bytes: 512 * 1024 * 1024,
            global_bytes: 2048 * 1024 * 1024,
        }
    }
}
impl MetricsBudgets {
    fn from_lookup(
        lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
    ) -> Result<Self, String> {
        fn value(
            lookup: &mut impl FnMut(&str) -> Result<Option<String>, String>,
            key: &str,
            default: u64,
            max: u64,
        ) -> Result<u64, String> {
            let n = lookup(key)?
                .map(|v| v.parse::<u64>().map_err(|_| format!("invalid {key}")))
                .transpose()?
                .unwrap_or(default);
            if n == 0 || n > max {
                return Err(format!("{key} must be in 1..={max}"));
            }
            Ok(n)
        }
        let result = Self {
            workers: value(lookup, "CHRONO_USAGE_METRICS_WORKERS", 2, 2)? as u8,
            queue: value(lookup, "CHRONO_USAGE_METRICS_QUEUE", 8, 8)?,
            source_calls: value(lookup, "CHRONO_USAGE_METRICS_SOURCE_CALLS", 2, 2)? as u8,
            source_seconds: value(lookup, "CHRONO_USAGE_METRICS_SOURCE_SECONDS", 5, 5)?,
            report_seconds: value(lookup, "CHRONO_USAGE_METRICS_REPORT_SECONDS", 60, 60)?,
            report_bytes: value(lookup, "CHRONO_USAGE_METRICS_REPORT_MIB", 256, 1024)? as i64
                * 1024
                * 1024,
            tenant_bytes: value(lookup, "CHRONO_USAGE_METRICS_TENANT_MIB", 512, 4096)? as i64
                * 1024
                * 1024,
            global_bytes: value(lookup, "CHRONO_USAGE_METRICS_GLOBAL_MIB", 2048, 16384)? as i64
                * 1024
                * 1024,
        };
        if result.report_bytes > result.tenant_bytes || result.tenant_bytes > result.global_bytes {
            return Err("usage metrics budgets require report <= tenant <= global".into());
        }
        Ok(result)
    }
}

#[cfg(test)]
#[path = "sandbox_metrics_tests.rs"]
mod tests;
