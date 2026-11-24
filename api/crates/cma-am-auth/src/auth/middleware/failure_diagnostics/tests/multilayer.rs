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
        let warmup = Log::default();
        let interest = tracing::Dispatch::new(tracing_subscriber::registry().with(layer(
            warmup.clone(),
            json,
            false,
            true,
            true,
        )));
        // Hold dynamic interest across all three classes. Other tests can
        // register or drop subscribers between calls; without this retained
        // dispatcher, a static filtered layer can emit only part of the group.
        tracing::dispatcher::with_default(&interest, emit_private_classes);
        for dynamic in [true, false] {
            for reverse in [true, false] {
                let (filtered, accepting) = capture(json, dynamic, reverse, true);
                // Like the other registered auth roots, an event whose root
                // this writer filters out is omitted rather than emitted
                // without its content-free scope.
                assert_private_output_with_root(&filtered, json, true, false);
                assert_private_output(&accepting, json, false);
            }
        }
        drop(interest);
        let warmup = String::from_utf8(warmup.0.lock().unwrap().clone()).unwrap();
        assert_private_output(&warmup, json, false);
    }
}

#[test]
fn dynamic_dispatch_does_not_override_static_root_exclusion() {
    for json in [true, false] {
        for exclude_root in [false, true] {
            let warmup = Log::default();
            let other = tracing::Dispatch::new(tracing_subscriber::registry().with(layer(
                warmup.clone(),
                json,
                false,
                true,
                true,
            )));
            // Retain a dynamic subscriber to force fresh event filtering even
            // after this subscriber has rejected the root. Warm the real
            // helper's callsites; no concurrent test execution is required.
            tracing::dispatcher::with_default(&other, emit_private_classes);
            let log = Log::default();
            let selected = dispatch(log.clone(), json, exclude_root, Filter::PerLayer);
            tracing::dispatcher::with_default(&selected, emit_private_classes);
            drop(selected);
            drop(other);
            let text = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
            assert_private_output(&text, json, exclude_root);
            let text = String::from_utf8(warmup.0.lock().unwrap().clone()).unwrap();
            assert_private_output(&text, json, false);
        }
    }
}

#[test]
fn stock_json_counterexample_requires_the_diagnostic_formatter() {
    for reverse in [true, false] {
        let (filtered, accepting) = capture(true, true, reverse, false);
        assert_eq!(filtered.lines().count(), 4);
        for line in filtered.lines() {
            let event: Value = serde_json::from_str(line).unwrap();
            assert_eq!(event["target"], crate::diagnostics::AUTHENTICATION_TARGET);
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
