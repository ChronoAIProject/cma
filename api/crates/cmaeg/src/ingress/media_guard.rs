//! The no-loss guarantee for media on relay and native Telegram ingress.
//!
//! An activity that fails validation is answered 400 on the relay path, which
//! NyxID never retries, and 401 on native Telegram, which makes Telegram
//! redeliver the same update. Extractors normalize every sender-declared fact
//! into the validated domain first; this guard is the tested fallback for a
//! validator rule the normalizer missed, so an attachment can degrade a message
//! or be refused with 202 `attachment_unsupported`, but never lose it. A failure
//! that remains without any attachment is unrelated to media and keeps its
//! existing answer.
//!
//! Custom ingress does not use the guard: its validation errors go back to the
//! sender synchronously, so the sender can correct its own attachment.
use crate::{
    ingress::{admission::Refusal, launch::LaunchRequest},
    telemetry::names,
};
use cmaeg_core::{
    activity::{Activity, ActivityKind, Attachment},
    ValidationError,
};

/// The activity to seal, or the deliberate refusal that replaces it. Boxed like
/// the other ingress outcomes, because a refusal is a single byte.
#[derive(Debug)]
pub(crate) enum Admit {
    Ok(Box<Activity>),
    Refused(Refusal),
}

/// What a source's launch step produced: a turn to start, or a refusal that the
/// HTTP layer answers with 202 so the platform never treats it as a failure.
#[derive(Debug)]
pub(crate) enum Built {
    Launch(Box<LaunchRequest>),
    Refused(Refusal),
}

/// Validate `activity` for `source` (a bounded metric label).
pub(crate) fn admit(activity: Activity, source: &'static str) -> Result<Admit, ValidationError> {
    admit_with(activity, source, Activity::validate)
}

/// [`admit`] with an injectable validator, so every fallback step is testable
/// even though normalized extractor output always passes the real one.
pub(crate) fn admit_with(
    mut activity: Activity,
    source: &'static str,
    validate: impl Fn(&Activity) -> Result<(), ValidationError>,
) -> Result<Admit, ValidationError> {
    let error = match validate(&activity) {
        Ok(()) => return Ok(Admit::Ok(Box::new(activity))),
        Err(error) => error,
    };
    let Some(attachments) = attachments_mut(&mut activity).filter(|list| !list.is_empty()) else {
        return Err(error);
    };
    // The pointer names the field; the value is sender-controlled and never logged.
    tracing::error!(
        source,
        pointer = %error.pointer,
        "activity failed validation after attachment normalization; degrading attachments"
    );
    metrics::counter!(names::MEDIA_ACTIVITY_DEGRADED, names::SOURCE => source).increment(1);
    for attachment in attachments.iter_mut() {
        *attachment = attachment.minimal();
    }
    let degraded = match validate(&activity) {
        Ok(()) => return Ok(Admit::Ok(Box::new(activity))),
        Err(degraded) => degraded,
    };
    if let Some(attachments) = attachments_mut(&mut activity) {
        attachments.clear();
    }
    if validate(&activity).is_ok() {
        tracing::warn!(
            source,
            pointer = %degraded.pointer,
            "activity still invalid with minimal attachments; refusing as attachment_unsupported"
        );
        return Ok(Admit::Refused(Refusal::AttachmentUnsupported));
    }
    Err(error)
}

fn attachments_mut(activity: &mut Activity) -> Option<&mut Vec<Attachment>> {
    match &mut activity.kind {
        ActivityKind::Message { attachments, .. } | ActivityKind::Event { attachments, .. } => {
            Some(attachments)
        }
        ActivityKind::Interaction(_) | ActivityKind::Command { .. } => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use metrics_exporter_prometheus::PrometheusBuilder;
    use serde_json::json;
    use std::sync::{Arc, Mutex};

    const FILENAME: &str = "private-holiday-plans.pdf";
    const POINTER: &str = "/kind/attachments/0/filename";
    const DEGRADED: &str = "cmaeg_media_activity_degraded_total{source=\"nyxid_relay\"}";

    fn activity(attachments: serde_json::Value) -> Activity {
        serde_json::from_value(json!({
            "schema_version": 1,
            "event_id": "message-1",
            "kind": {"type": "message", "text": "hello", "attachments": attachments, "mentions_bot": false},
            "actor": {"id": "7", "kind": "human"},
            "conversation": {"id": "42", "kind": "private"},
            "occurred_at": "2026-10-07T00:00:00Z",
            "source": {"type": "nyxid_relay", "platform": "telegram", "route_id": "route-1"}
        }))
        .unwrap()
    }

    fn with_file() -> Activity {
        activity(json!([{
            "reference": "telegram:AgADfixture",
            "kind": "file",
            "filename": FILENAME,
            "mime_type": "application/pdf",
            "size_bytes": 20480,
            "variant": "document",
            "source": {"type": "nyxid_relay", "index": 0}
        }]))
    }

    fn attachments(activity: &Activity) -> &[Attachment] {
        match &activity.kind {
            ActivityKind::Message { attachments, .. } | ActivityKind::Event { attachments, .. } => {
                attachments
            }
            _ => &[],
        }
    }

    #[derive(Clone, Default)]
    struct Logs(Arc<Mutex<Vec<u8>>>);
    impl std::io::Write for Logs {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Runs `admit_with` with local metrics and logs: (result, metrics, logs).
    fn observe(
        activity: Activity,
        validate: impl Fn(&Activity) -> Result<(), ValidationError>,
    ) -> (Result<Admit, ValidationError>, String, String) {
        let recorder = PrometheusBuilder::new().build_recorder();
        let handle = recorder.handle();
        let logs = Logs::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .without_time()
            .with_writer(move || writer.clone())
            .finish();
        let result = tracing::subscriber::with_default(subscriber, || {
            metrics::with_local_recorder(&recorder, || {
                admit_with(activity, "nyxid_relay", validate)
            })
        });
        let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        (result, handle.render(), logs)
    }

    fn degraded(metrics: &str) -> u64 {
        metrics
            .lines()
            .find_map(|line| line.strip_prefix(DEGRADED)?.trim().parse().ok())
            .unwrap_or(0)
    }

    fn refuse_filenames(activity: &Activity) -> Result<(), ValidationError> {
        if attachments(activity)
            .iter()
            .any(|attachment| attachment.filename.is_some())
        {
            return Err(ValidationError::new(POINTER, "rule the normalizer missed"));
        }
        Ok(())
    }

    #[test]
    fn a_valid_activity_is_admitted_unchanged() {
        let original = with_file();
        let (result, metrics, logs) = observe(original.clone(), Activity::validate);
        assert!(matches!(result, Ok(Admit::Ok(activity)) if *activity == original));
        assert_eq!(degraded(&metrics), 0);
        assert!(logs.is_empty(), "{logs}");
        assert!(matches!(admit(with_file(), "telegram"), Ok(Admit::Ok(_))));
    }

    #[test]
    fn an_invalid_fact_degrades_attachments_to_minimal_and_logs_only_the_pointer() {
        let original = with_file();
        let (result, metrics, logs) = observe(original.clone(), refuse_filenames);
        let Ok(Admit::Ok(activity)) = result else {
            panic!("expected a degraded admission, got {result:?}")
        };
        assert_eq!(
            attachments(&activity),
            [attachments(&original)[0].minimal()]
        );
        assert_eq!(attachments(&activity)[0].reference, "telegram:AgADfixture");
        assert!(attachments(&activity)[0].source.is_some());
        assert_eq!(degraded(&metrics), 1);
        assert!(logs.contains("degrading attachments"), "{logs}");
        assert!(logs.contains(POINTER), "{logs}");
        assert!(logs.contains("source=\"nyxid_relay\""), "{logs}");
        assert!(!logs.contains(FILENAME), "{logs}");
    }

    #[test]
    fn event_attachments_are_degraded_too() {
        let mut event = with_file();
        let files = attachments(&event).to_vec();
        event.kind = ActivityKind::Event {
            ce: serde_json::from_value(
                json!({"specversion": "1.0", "id": "e-1", "type": "t", "source": "s"}),
            )
            .unwrap(),
            attachments: files,
        };
        let (result, metrics, _) = observe(event, refuse_filenames);
        let Ok(Admit::Ok(activity)) = result else {
            panic!("expected a degraded admission, got {result:?}")
        };
        assert!(attachments(&activity)[0].filename.is_none());
        assert_eq!(degraded(&metrics), 1);
    }

    #[test]
    fn an_attachment_that_still_fails_when_minimal_is_refused_as_unsupported() {
        let refuse_any = |activity: &Activity| {
            if attachments(activity).is_empty() {
                Ok(())
            } else {
                Err(ValidationError::new(
                    "/kind/attachments/0/source",
                    "refused",
                ))
            }
        };
        let (result, metrics, logs) = observe(with_file(), refuse_any);
        assert!(matches!(
            result,
            Ok(Admit::Refused(Refusal::AttachmentUnsupported))
        ));
        assert_eq!(degraded(&metrics), 1);
        assert!(
            logs.contains("refusing as attachment_unsupported"),
            "{logs}"
        );
        assert!(!logs.contains(FILENAME), "{logs}");
    }

    #[test]
    fn a_failure_unrelated_to_media_keeps_the_original_error() {
        let always = |_: &Activity| Err(ValidationError::new("/actor/id", "always"));
        let (result, metrics, _) = observe(with_file(), always);
        let error = result.unwrap_err();
        assert_eq!(error.pointer, "/actor/id");
        assert_eq!(error.message, "always");
        assert_eq!(degraded(&metrics), 1);
    }

    #[test]
    fn an_attachment_free_failure_is_not_degraded() {
        let always = |_: &Activity| Err(ValidationError::new("/kind/text", "too long"));
        let (result, metrics, logs) = observe(activity(json!([])), always);
        assert_eq!(result.unwrap_err().pointer, "/kind/text");
        assert_eq!(degraded(&metrics), 0);
        assert!(logs.is_empty(), "{logs}");
        let mut command = activity(json!([]));
        command.kind = ActivityKind::Command {
            name: "start".into(),
            args: String::new(),
        };
        let (result, metrics, _) = observe(command, always);
        assert_eq!(result.unwrap_err().pointer, "/kind/text");
        assert_eq!(degraded(&metrics), 0);
    }
}
