use std::io::{self, Cursor};

use tokio_tungstenite::tungstenite::protocol::frame::FrameHeader;

// RFC 6455's two-byte prefix, eight-byte extended length and four-byte mask.
const HEADER_BYTES: usize = 14;

#[derive(Default)]
pub(super) struct HeaderGate {
    bytes: [u8; HEADER_BYTES],
    length: usize,
    emitted: usize,
    payload: Option<u64>,
}

impl HeaderGate {
    pub(super) fn begin(&mut self) {
        if self.payload == Some(0) && self.emitted == self.length {
            *self = Self::default();
        }
    }

    pub(super) fn needs_header(&self) -> bool {
        self.payload.is_none()
    }

    pub(super) fn parse(&self) -> io::Result<Option<(FrameHeader, u64)>> {
        FrameHeader::parse(&mut Cursor::new(&self.bytes[..self.length]))
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    pub(super) fn unfilled_header(&mut self) -> io::Result<&mut [u8]> {
        let end = if self.length < 2 {
            2
        } else {
            let extended = match self.bytes[1] & 0x7f {
                126 => 2,
                127 => 8,
                _ => 0,
            };
            2 + extended + if self.bytes[1] & 0x80 == 0 { 0 } else { 4 }
        };
        if self.length >= end {
            return Err(io::ErrorKind::InvalidData.into());
        }
        Ok(&mut self.bytes[self.length..end])
    }

    pub(super) fn received_header(&mut self, count: usize) {
        self.length += count;
    }

    pub(super) fn admit(&mut self, payload: u64) {
        self.payload = Some(payload);
    }

    pub(super) fn is_partial(&self) -> bool {
        self.length != 0
    }

    pub(super) fn header(&self) -> &[u8] {
        &self.bytes[self.emitted..self.length]
    }

    pub(super) fn emitted_header(&mut self, count: usize) {
        self.emitted += count;
    }

    pub(super) fn payload_chunk(&self, capacity: usize) -> usize {
        self.payload.unwrap_or(0).min(capacity as u64) as usize
    }

    pub(super) fn emitted_payload(&mut self, count: usize) {
        *self
            .payload
            .as_mut()
            .expect("payload follows admitted header") -= count as u64;
    }
}
