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
    let subscriber = tracing_subscriber::fmt()
        .json()
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
    let rows: Vec<serde_json::Value> = text
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(rows.len(), 3);
    for row in &rows {
        assert_eq!(row["target"], "cma_startup");
        assert!(row.get("span").is_none());
        assert!(row.get("spans").is_none());
    }
    assert_eq!(rows[0]["fields"]["item_count"], 34);
    assert_eq!(rows[0]["fields"]["count_known"], true);
    assert!(rows[0]["fields"]["elapsed_ms"].as_u64().unwrap() >= 3000);
    assert_eq!(rows[1]["fields"]["outcome"], "error");
    assert_eq!(rows[1]["fields"]["count_known"], false);
    assert_eq!(rows[2]["fields"]["outcome"], "cancelled");
    assert_eq!(rows[2]["fields"]["count_known"], false);
}
