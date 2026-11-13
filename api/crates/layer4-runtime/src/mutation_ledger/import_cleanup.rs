//! Preserve the primary refusal without silently losing a close failure.
use crate::failure::Result;

pub(super) fn finish<T>(outcome: Result<T>, closed: Result<()>, phase: &'static str) -> Result<T> {
    match (outcome, closed) {
        (Err(original), Err(close)) => {
            tracing::error!(phase, code = %close.code, "held ledger import cleanup failed after refusal");
            Err(original)
        }
        (Err(original), Ok(())) => Err(original),
        (Ok(_), Err(close)) => Err(close),
        (Ok(value), Ok(())) => Ok(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::failure::RuntimeError;
    use std::io::Write;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn import_cleanup_reports_secondary_failure_and_preserves_original_refusal() {
        let captured = Capture::default();
        let writer = captured.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            for phase in ["initialize", "stage", "verify"] {
                let original =
                    RuntimeError::rejected("mutation_ledger_invalid", "private original");
                let close = RuntimeError::rejected("mutation_ledger_storage_hold", "private close");
                let error = finish::<()>(Err(original), Err(close), phase).unwrap_err();
                assert_eq!(error.code, "mutation_ledger_invalid");
                assert_eq!(error.detail, "private original");
                assert!(!error.retryable);
            }
        });
        let text = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
        for phase in ["initialize", "stage", "verify"] {
            assert!(text.contains(phase), "{text}");
        }
        assert!(text.contains("mutation_ledger_storage_hold"));
        assert!(!text.contains("private"));
    }

    #[test]
    fn import_cleanup_never_reports_success_after_a_close_failure() {
        let close = || RuntimeError::rejected("mutation_ledger_storage_hold", "close refused");
        assert_eq!(
            finish(Ok(7), Err(close()), "verify").unwrap_err().code,
            close().code
        );
        assert_eq!(
            finish::<()>(Err(close()), Ok(()), "stage")
                .unwrap_err()
                .code,
            close().code
        );
        assert_eq!(finish(Ok(7), Ok(()), "verify").unwrap(), 7);
    }
}
