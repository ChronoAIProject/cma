//! Explicit local collectors: tests never mutate process-wide environment/state.
use super::{
    output::{Inputs, Message, Output},
    record::Record,
    ring::Snapshot,
    Recorder,
};
use std::{
    io::Write,
    sync::{Arc, Mutex},
};

pub(crate) struct Capture {
    pub(crate) recorder: Arc<Recorder>,
    inputs: Inputs,
}
impl Capture {
    pub(crate) fn new() -> Self {
        let (output, inputs) = Output::channel();
        Self {
            recorder: Recorder::new(output),
            inputs,
        }
    }
    pub(crate) fn records(&self) -> Vec<Record> {
        self.inputs
            .records
            .try_iter()
            .filter_map(|e| match e.item {
                Message::Record(record) => Some(record),
                Message::Event(_) => None,
            })
            .collect()
    }
    pub(crate) fn snapshots(&self) -> Vec<Snapshot> {
        self.inputs.snapshots.try_iter().map(|e| e.item).collect()
    }
    pub(crate) fn logs(&self) -> String {
        let output = Bytes::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .with_current_span(false)
            .with_span_list(false)
            .without_time()
            .with_writer(output.clone())
            .finish();
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("private", identity = "do-not-retain-this-value");
            let _entered = span.enter();
            for e in self.inputs.records.try_iter() {
                e.write();
            }
            for e in self.inputs.events.try_iter() {
                e.write();
            }
            for e in self.inputs.snapshots.try_iter() {
                e.item.write(&e.process, 0);
            }
        });
        let data = output.0.lock().unwrap().clone();
        String::from_utf8(data).unwrap()
    }
}
#[derive(Clone, Default)]
struct Bytes(Arc<Mutex<Vec<u8>>>);
impl Write for Bytes {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Bytes {
    type Writer = Self;
    fn make_writer(&'a self) -> Self {
        self.clone()
    }
}
