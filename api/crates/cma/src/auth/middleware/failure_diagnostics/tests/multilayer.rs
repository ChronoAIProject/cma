use super::*;
use tracing_subscriber::registry::LookupSpan;

fn layer<S>(
    log: Log,
    json: bool,
    exclude_root: bool,
    dynamic: bool,
    private_format: bool,
) -> Box<dyn Layer<S> + Send + Sync>
where
    S: tracing::Subscriber + for<'a> LookupSpan<'a> + 'static,
{
    let layer = tracing_subscriber::fmt::layer()
        .without_time()
        .with_ansi(false)
        .with_writer(move || log.clone());
    let layer = if json {
        if private_format {
            layer
                .json()
                .event_format(crate::diagnostic_format::json())
                .boxed()
        } else {
            layer.json().boxed()
        }
    } else {
        layer.event_format(crate::diagnostic_format::text()).boxed()
    };
    if dynamic {
        layer
            .with_filter(tracing_subscriber::filter::dynamic_filter_fn(
                move |metadata, _| !exclude_root || metadata.name() != "authentication_failure",
            ))
            .boxed()
    } else {
        layer
            .with_filter(tracing_subscriber::filter::filter_fn(move |metadata| {
                !exclude_root || metadata.name() != "authentication_failure"
            }))
            .boxed()
    }
}

fn capture(json: bool, dynamic: bool, reverse: bool, private_format: bool) -> (String, String) {
    let filtered = Log::default();
    let accepting = Log::default();
    let subscriber = if reverse {
        tracing::Dispatch::new(
            tracing_subscriber::registry()
                .with(layer(
                    accepting.clone(),
                    json,
                    false,
                    dynamic,
                    private_format,
                ))
                .with(layer(filtered.clone(), json, true, dynamic, private_format)),
        )
    } else {
        tracing::Dispatch::new(
            tracing_subscriber::registry()
                .with(layer(filtered.clone(), json, true, dynamic, private_format))
                .with(layer(
                    accepting.clone(),
                    json,
                    false,
                    dynamic,
                    private_format,
                )),
        )
    };
    // Two output layers of one subscriber, with their own buffers and filters.
    // Dynamic filters recompute event enablement after the span metadata guard;
    // no other subscriber, concurrent thread or callsite warming is required.
    tracing::dispatcher::with_default(&subscriber, emit_private_classes);
    let text = |log: &Log| String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    (text(&filtered), text(&accepting))
}

#[test]
fn independent_output_layers_keep_all_typed_fields_private() {
    for json in [true, false] {
        for dynamic in [true, false] {
            for reverse in [true, false] {
                let (filtered, accepting) = capture(json, dynamic, reverse, true);
                // Static filters can suppress this output; other live dispatchers
                // can change shared callsite interest and force a fresh event check.
                // Dynamic filters always check the event after rejecting its root.
                assert_private_output_with_root(
                    &filtered,
                    json,
                    !dynamic && filtered.is_empty(),
                    false,
                );
                assert_private_output(&accepting, json, false);
            }
        }
    }
}

#[test]
fn stock_json_counterexample_requires_the_diagnostic_formatter() {
    for reverse in [true, false] {
        let (filtered, accepting) = capture(true, true, reverse, false);
        assert_eq!(filtered.lines().count(), 3);
        for line in filtered.lines() {
            let event: Value = serde_json::from_str(line).unwrap();
            assert_eq!(
                event["target"],
                crate::diagnostic_format::AUTHENTICATION_TARGET
            );
            assert_eq!(event["span"]["secret"], "must-not-leak-span");
        }
        assert!(!accepting.contains("must-not-leak"));
        for private in [
            "must-not-leak-error",
            "must-not-leak-body",
            "123456",
            "654321",
        ] {
            assert!(!filtered.contains(private));
        }
    }
}
