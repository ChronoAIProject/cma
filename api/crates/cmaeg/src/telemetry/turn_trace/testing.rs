use super::{writer, Writer};
use std::{
    io::{self, Write},
    sync::mpsc,
};
use tracing_appender::non_blocking::WorkerGuard;

thread_local! {
    static TEST_WRITER: std::cell::RefCell<Option<Writer>> = const { std::cell::RefCell::new(None) };
}

pub(super) fn writer() -> Option<Writer> {
    TEST_WRITER.with(|slot| slot.borrow().clone())
}

/// Scoped injection for current-thread delivery tests, without changing the
/// process environment or another concurrently executing test's writer.
pub(crate) fn install(writer: Writer) -> TestWriter {
    TestWriter(TEST_WRITER.with(|slot| slot.replace(Some(writer))))
}

pub(crate) struct TestWriter(Option<Writer>);

impl Drop for TestWriter {
    fn drop(&mut self) {
        TEST_WRITER.with(|slot| slot.replace(self.0.take()));
    }
}

struct Capture(mpsc::Sender<Vec<u8>>);
impl Write for Capture {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0
            .send(bytes.to_vec())
            .map_err(|_| io::ErrorKind::BrokenPipe)?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(crate) fn capture() -> (Writer, mpsc::Receiver<Vec<u8>>, WorkerGuard) {
    let (sender, receiver) = mpsc::channel();
    let output = writer::start(Capture(sender)).unwrap();
    (output.writer, receiver, output.guard)
}
