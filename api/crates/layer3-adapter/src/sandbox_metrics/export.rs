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
    let mut named = BTreeMap::new();
    for series in decode_series(body, selection, |left, right| left == right)? {
        let chart = PhysicalSeries {
            metric: series.metric,
            name: series.name,
            labels: series.labels,
            samples: series
                .samples
                .into_iter()
                .map(|sample| Sample {
                    at: sample.timestamp_ms as f64 / 1000.0,
                    value: sample.value,
                })
                .collect(),
        };
        if named.insert(chart.name.clone(), chart).is_some() {
            return Err(invalid());
        }
    }
    Ok(named.into_values().collect())
}

pub fn decode_raw_export(body: &[u8], selection: &Selection) -> Result<Vec<RawPhysicalSeries>> {
    decode_series(body, selection, |left, right| {
        left.to_bits() == right.to_bits()
    })
}

fn decode_series(
    body: &[u8],
    selection: &Selection,
    same_value: impl Fn(f64, f64) -> bool,
) -> Result<Vec<RawPhysicalSeries>> {
    selection.validate()?;
    if body.len() > super::transport::MAX_RESPONSE_BYTES {
        return Err(invalid());
    }
    let from_ms = selection.from.checked_mul(1000).ok_or_else(invalid)?;
    let to_ms = selection.to.checked_mul(1000).ok_or_else(invalid)?;
    let mut series: BTreeMap<BTreeMap<String, String>, RawPhysicalSeries> = BTreeMap::new();
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
        // Split export lines for the same label set can be combined. Separate
        // physical incarnations can share the exported series name.
        let entry = series
            .entry(row.metric.clone())
            .or_insert_with(|| RawPhysicalSeries {
                metric: selection.metric,
                name: name.clone(),
                labels: row.metric.clone(),
                samples: Vec::new(),
            });
        for (timestamp_ms, value) in row.timestamps.into_iter().zip(row.values) {
            if !value.is_finite() || value < 0.0 || timestamp_ms < from_ms || timestamp_ms > to_ms {
                return Err(invalid());
            }
            entry.samples.push(RawSample {
                timestamp_ms,
                value,
            });
        }
    }
    for entry in series.values_mut() {
        entry.samples.sort_by_key(|sample| sample.timestamp_ms);
        if entry.samples.windows(2).any(|pair| {
            pair[0].timestamp_ms == pair[1].timestamp_ms
                && !same_value(pair[0].value, pair[1].value)
        }) {
            return Err(invalid());
        }
        entry.samples.dedup_by_key(|sample| sample.timestamp_ms);
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

    #[test]
    fn raw_export_preserves_milliseconds_and_container_identity_for_reused_names() {
        let mut selected = selection();
        selected.from = 9_007_199_254_740;
        selected.to = selected.from + 2;
        let timestamp = 9_007_199_254_740_993_u64;
        let row = |container, timestamp| {
            serde_json::json!({
                "metric": {
                    "__name__": selected.metric.name(), "cluster": selected.cluster,
                    "namespace": selected.namespace, "pod": selected.names[0],
                    "job": "kubelet", "metrics_path": "/metrics/resource", "container": "runtime",
                    "pod_uid": "fixture-pod", "container_id": container,
                },
                "timestamps": [timestamp], "values": [3.25],
            })
        };
        let raw = [
            row("one", timestamp),
            row("two", timestamp + 1),
            row("one", timestamp),
        ]
        .into_iter()
        .map(|row| row.to_string())
        .collect::<Vec<_>>()
        .join("\n");
        let parsed = decode_raw_export(raw.as_bytes(), &selected).unwrap();
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].labels["container_id"], "one");
        assert_eq!(
            parsed[0].samples,
            vec![RawSample {
                timestamp_ms: timestamp,
                value: 3.25
            }]
        );
        assert_eq!(parsed[1].samples[0].timestamp_ms, timestamp + 1);
        assert!(decode_export(raw.as_bytes(), &selected).is_err());
    }

    #[test]
    fn raw_export_refuses_conflicting_value_bits_in_either_order() {
        let make_row = |value| {
            serde_json::json!({
                "metric": {
                    "__name__": "container_cpu_usage_seconds_total", "cluster": "prod",
                    "namespace": "runtime", "pod": "cold-claim", "container": "runtime",
                    "job": "kubelet", "metrics_path": "/metrics/resource",
                },
                "timestamps": [1250], "values": [value],
            })
        };
        let first = make_row(0.0).to_string();
        let second = make_row(-0.0).to_string();
        for input in [format!("{first}\n{second}"), format!("{second}\n{first}")] {
            assert!(decode_raw_export(input.as_bytes(), &selection()).is_err());
            assert!(decode_export(input.as_bytes(), &selection()).is_ok());
        }
    }

    #[test]
    fn raw_export_rejects_mislabeled_or_incomplete_source() {
        let row = serde_json::json!({
            "metric": {
                "__name__": "container_cpu_usage_seconds_total", "cluster": "prod",
                "namespace": "runtime", "pod": "cold-claim", "container": "runtime",
                "job": "kubelet", "metrics_path": "/metrics/resource",
            },
            "timestamps": [1250], "values": [3.25],
        });
        for missing in ["__name__", "pod", "job", "metrics_path", "container"] {
            let mut incomplete = row.clone();
            incomplete["metric"]
                .as_object_mut()
                .unwrap()
                .remove(missing);
            assert!(decode_raw_export(incomplete.to_string().as_bytes(), &selection()).is_err());
        }
        let mut selected = selection();
        selected.metric = SourceMetric::WorkingSet;
        assert!(decode_raw_export(row.to_string().as_bytes(), &selected).is_err());
    }
}
