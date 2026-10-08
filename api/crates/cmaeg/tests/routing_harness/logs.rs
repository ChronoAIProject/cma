//! Local capture for the current-thread gateway and its delivery tasks.
use std::{
    io::Write,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
pub struct Logs(Arc<Mutex<Vec<u8>>>);
impl Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl Logs {
    pub fn capture(&self) -> tracing::subscriber::DefaultGuard {
        let writer = self.clone();
        tracing::subscriber::set_default(
            tracing_subscriber::fmt()
                .with_ansi(false)
                .without_time()
                .with_writer(move || writer.clone())
                .finish(),
        )
    }
    pub fn text(&self) -> String {
        String::from_utf8(self.0.lock().unwrap().clone()).unwrap()
    }
}
