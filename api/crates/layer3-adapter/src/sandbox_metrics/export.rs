use super::*;
use crate::{AdapterError, Result};

pub fn selector(selection: &Selection) -> Result<String> {
    selection.validate()?;
    let names = selection
        .names
        .iter()
        .map(|v| v.replace('.', "\\."))
        .collect::<Vec<_>>()
        .join("|");
    // Values are backend-created DNS labels. JSON escaping also protects the
    // regex backslash when embedded in a Prometheus string literal.
    let quoted_names = serde_json::to_string(&format!("^(?:{names})$")).map_err(|_| invalid())?;
    let container = if selection.metric.is_pvc() {
        ""
    } else {
        ",container=\"runtime\""
    };
    Ok(format!(
        "{}{{cluster=\"{}\",namespace=\"{}\",job=\"kubelet\",metrics_path=\"{}\",{}=~{}{}}}",
        selection.metric.name(),
        selection.cluster,
        selection.namespace,
        selection.metric.metrics_path(),
        selection.metric.identity_label(),
        quoted_names,
        container
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Export {
    metric: BTreeMap<String, String>,
    values: Vec<f64>,
    timestamps: Vec<u64>,
}

pub fn decode_export(body: &[u8], selection: &Selection) -> Result<Vec<PhysicalSeries>> {
    selection.validate()?;
    if body.len() > super::transport::MAX_RESPONSE_BYTES {
        return Err(invalid());
    }
    let mut series: BTreeMap<String, PhysicalSeries> = BTreeMap::new();
    for line in body.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
        let row: Export = serde_json::from_slice(line).map_err(|_| invalid())?;
        let name = row
            .metric
            .get(selection.metric.identity_label())
            .ok_or_else(invalid)?;
        if row.metric.get("__name__").map(String::as_str) != Some(selection.metric.name())
            || row.metric.get("cluster") != Some(&selection.cluster)
            || row.metric.get("namespace") != Some(&selection.namespace)
            || row.metric.get("job").map(String::as_str) != Some("kubelet")
            || row.metric.get("metrics_path").map(String::as_str)
                != Some(selection.metric.metrics_path())
            || (!selection.metric.is_pvc()
                && row.metric.get("container").map(String::as_str) != Some("runtime"))
            || !selection.names.contains(name)
            || row.values.len() != row.timestamps.len()
            || row.values.len() > 300_000
        {
            return Err(invalid());
        }
        let entry = series
            .entry(name.clone())
            .or_insert_with(|| PhysicalSeries {
                metric: selection.metric,
                name: name.clone(),
                labels: row.metric.clone(),
                samples: Vec::new(),
            });
        // Export may split one physical series into several JSON lines. Another
        // job/replica/instance is ambiguity, never an extra value to sum.
        if entry.labels != row.metric {
            return Err(invalid());
        }
        for (at, value) in row.timestamps.into_iter().zip(row.values) {
            let at = at as f64 / 1000.0;
            if !value.is_finite()
                || value < 0.0
                || at < selection.from as f64
                || at > selection.to as f64
            {
                return Err(invalid());
            }
            entry.samples.push(Sample { at, value });
        }
    }
    for entry in series.values_mut() {
        entry.samples.sort_by(|a, b| a.at.total_cmp(&b.at));
        if entry
            .samples
            .windows(2)
            .any(|w| w[0].at == w[1].at && w[0].value != w[1].value)
        {
            return Err(invalid());
        }
        entry.samples.dedup_by(|a, b| a.at == b.at);
    }
    Ok(series.into_values().collect())
}
fn invalid() -> AdapterError {
    AdapterError::Unavailable("monitoring response is incomplete, ambiguous or invalid".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn selection() -> Selection {
        Selection {
            cluster: "prod".into(),
            namespace: "runtime".into(),
            names: vec!["cold-claim".into()],
            metric: SourceMetric::CpuCounter,
            from: 1,
            to: 60,
        }
    }
    #[test]
    fn selectors_are_exact_and_never_accept_query_language() {
        let mut s = selection();
        assert!(selector(&s).unwrap().contains("container=\"runtime\""));
        assert!(selector(&s).unwrap().contains("^(?:cold-claim)$"));
        for v in [".*", "x\"} or up", "a|b", "x\\y", "a,b"] {
            s.names = vec![v.into()];
            assert!(selector(&s).is_err());
        }
    }
    #[test]
    fn raw_timestamps_are_preserved_and_cadvisor_is_refused() {
        let mut row = serde_json::json!({"metric":{"__name__":"container_cpu_usage_seconds_total","cluster":"prod","namespace":"runtime","pod":"cold-claim","container":"runtime","job":"kubelet","metrics_path":"/metrics/resource"},"timestamps":[1250,30500],"values":[2,4]});
        let bytes = serde_json::to_vec(&row).unwrap();
        assert_eq!(
            decode_export(&bytes, &selection()).unwrap()[0].samples[0].at,
            1.25
        );
        row["metric"]["metrics_path"] = "/metrics/cadvisor".into();
        assert!(decode_export(&serde_json::to_vec(&row).unwrap(), &selection()).is_err());
        assert!(
            decode_export(b"{\"status\":\"success\",\"isPartial\":true}", &selection()).is_err()
        );
    }
}
