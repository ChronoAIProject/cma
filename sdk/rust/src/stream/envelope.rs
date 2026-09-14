use crate::{Error, Result};
use std::collections::VecDeque;

/// Track explicit IDs separately from Eventsource's inherited last-event ID.
#[derive(Default)]
pub(super) struct Envelopes {
    line: Vec<u8>,
    previous_cr: bool,
    first_line: bool,
    initialized: bool,
    data: bool,
    id: Option<String>,
    ready: VecDeque<Option<String>>,
}
impl Envelopes {
    pub(super) fn inspect(&mut self, bytes: &[u8]) -> Result<()> {
        for &byte in bytes {
            if byte == b'\n' && self.previous_cr {
                self.previous_cr = false;
                continue;
            }
            self.previous_cr = byte == b'\r';
            if matches!(byte, b'\r' | b'\n') {
                self.finish_line()?;
            } else {
                self.line.push(byte);
            }
        }
        Ok(())
    }
    fn finish_line(&mut self) -> Result<()> {
        if !self.initialized {
            self.first_line = true;
            self.initialized = true;
        }
        let mut bytes = std::mem::take(&mut self.line);
        if self.first_line {
            self.first_line = false;
            if bytes.starts_with(&[0xef, 0xbb, 0xbf]) {
                bytes.drain(..3);
            }
        }
        let line = std::str::from_utf8(&bytes)
            .map_err(|_| Error::protocol("event stream is not UTF-8"))?;
        if line.is_empty() {
            if self.data {
                self.ready.push_back(self.id.take());
            }
            self.data = false;
            self.id = None;
        } else {
            let (name, value) = line.split_once(':').unwrap_or((line, ""));
            let value = value.strip_prefix(' ').unwrap_or(value);
            if name == "data" {
                self.data = true;
            }
            if name == "id" && !value.contains('\0') {
                self.id = Some(value.to_owned());
            }
        }
        Ok(())
    }
    pub(super) fn next(&mut self) -> Result<Option<String>> {
        self.ready
            .pop_front()
            .ok_or_else(|| Error::protocol("event envelope tracking disagreed with SSE framing"))
    }
}
