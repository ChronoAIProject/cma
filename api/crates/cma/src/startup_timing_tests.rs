use super::*;
use std::{
    io::Write,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
struct Output(Arc<Mutex<Vec<u8>>>);
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Output {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}

#[test]
fn startup_phase_counts_and_outcomes_do_not_inherit_caller_payload() {
    let output = Output::default();
    // Production uses the text formatter, which honors explicit root events.
    // tracing-subscriber's JSON formatter can include an ambient span anyway.
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(output.clone())
        .finish();
    tracing::subscriber::with_default(subscriber, || {
        let parent = tracing::info_span!("caller", content = "startup-private-canary");
        let _entered = parent.enter();
        let mut phase = Phase::new("binding_reconciliation");
        phase.started = Instant::now() - std::time::Duration::from_secs(3);
        phase.finish("ok", Some(34));
        Phase::new("composition").finish("error", None);
        drop(Phase::new("runtime_inventory"));
    });
    let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
    assert!(!text.contains("startup-private-canary"));
    let rows: Vec<_> = text.lines().collect();
    assert_eq!(rows.len(), 3);
    for row in &rows {
        assert!(row.contains("INFO cma_startup:"));
        assert!(!row.contains("caller"));
    }
    assert!(rows[0].contains("item_count=34 count_known=true"));
    let elapsed: u64 = rows[0]
        .split_whitespace()
        .find_map(|field| field.strip_prefix("elapsed_ms="))
        .unwrap()
        .parse()
        .unwrap();
    assert!(elapsed >= 3000);
    assert!(rows[1].contains("outcome=\"error\""));
    assert!(rows[1].contains("item_count=0 count_known=false"));
    assert!(rows[2].contains("outcome=\"cancelled\""));
    assert!(rows[2].contains("item_count=0 count_known=false"));
}

#[test]
fn managed_machine_scope_startup_diagnostic_is_bounded() {
    let canary = uuid::Uuid::new_v4();
    for origin_enabled in [false, true] {
        for (scope, mode) in [
            (ManagedMachineUpdates::Disabled, "disabled"),
            (ManagedMachineUpdates::Canary(canary), "canary"),
            (ManagedMachineUpdates::All, "all"),
        ] {
            let output = Output::default();
            let subscriber = tracing_subscriber::fmt()
                .with_ansi(false)
                .without_time()
                .with_writer(output.clone())
                .finish();
            tracing::subscriber::with_default(subscriber, || {
                let parent = tracing::info_span!("caller", content = "startup-private-canary");
                let _entered = parent.enter();
                observe_managed_machine_scope(scope, origin_enabled);
            });
            let text = String::from_utf8(output.0.lock().unwrap().clone()).unwrap();
            let (level, message) = if origin_enabled && scope == ManagedMachineUpdates::Disabled {
                (
                    "WARN",
                    "Disabled Machine scope blocks origin qualification and new managed Machine update admission",
                )
            } else {
                ("INFO", "Managed Machine runtime update scope")
            };
            assert_eq!(text.trim(), format!(
                "{level} cma_startup: {message} setting=\"CHRONO_MANAGED_MACHINE_RUNTIME_UPDATES\" mode=\"{mode}\" runtime_image_origin_enabled={origin_enabled}"
            ));
            assert!(!text.contains(&canary.to_string()));
            assert!(!text.contains(&canary.simple().to_string()));
            assert!(!text.contains("startup-private-canary"));
        }
    }
}
