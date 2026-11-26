use reqwest::Response;
use serde_json::Value;

use crate::model::Reason;

use super::http::{content_type, status, MAX_BODY};

const MAX_FRAME: usize = 128 * 1024;
const MAX_FRAMES: usize = 2048;

pub struct Frame {
    pub event: Option<String>,
    pub id: Option<String>,
    pub value: Value,
}

pub struct Stream {
    response: Response,
    buffered: Vec<u8>,
    cursor: usize,
    consumed: usize,
    frames: usize,
}

impl Stream {
    pub fn open(response: Response) -> Result<Self, Reason> {
        status(&response)?;
        if content_type(&response) != Some("text/event-stream") {
            return Err(Reason::Contract);
        }
        Ok(Self {
            response,
            buffered: Vec::new(),
            cursor: 0,
            consumed: 0,
            frames: 0,
        })
    }

    pub async fn next(&mut self) -> Result<Option<Frame>, Reason> {
        let mut event = None;
        let mut id = None;
        let mut data = String::new();
        let mut frame_bytes = 0;
        loop {
            let Some(line) = self.line().await? else {
                return if frame_bytes == 0 {
                    Ok(None)
                } else {
                    Err(Reason::DeliveryLost)
                };
            };
            frame_bytes += line.len() + 1;
            if frame_bytes > MAX_FRAME {
                return Err(Reason::Contract);
            }
            if line.is_empty() {
                if data.is_empty() {
                    event = None;
                    id = None;
                    frame_bytes = 0;
                    continue;
                }
                self.frames += 1;
                if self.frames > MAX_FRAMES {
                    return Err(Reason::Contract);
                }
                let value = serde_json::from_str(&data).map_err(|_| Reason::Contract)?;
                return Ok(Some(Frame { event, id, value }));
            }
            if let Some((name, value)) = line.split_once(':') {
                let value = value.strip_prefix(' ').unwrap_or(value);
                match name {
                    "event" if !value.is_empty() => event = Some(value.to_owned()),
                    "id" if !value.contains('\0') => id = Some(value.to_owned()),
                    "data" => {
                        data.push_str(value);
                        data.push('\n');
                    }
                    _ => {}
                }
            }
        }
    }

    async fn line(&mut self) -> Result<Option<&str>, Reason> {
        let mut searched = self.cursor;
        loop {
            if let Some(relative) = self.buffered[searched..].iter().position(|b| *b == b'\n') {
                let end = searched + relative;
                let start = self.cursor;
                self.cursor = end + 1;
                let bytes = &self.buffered[start..end];
                let bytes = bytes.strip_suffix(b"\r").unwrap_or(bytes);
                return std::str::from_utf8(bytes)
                    .map(Some)
                    .map_err(|_| Reason::Contract);
            }
            if self.buffered.len() - self.cursor >= MAX_FRAME {
                return Err(Reason::Contract);
            }
            if self.cursor != 0 {
                self.buffered.drain(..self.cursor);
                self.cursor = 0;
            }
            searched = self.buffered.len();
            let chunk = self
                .response
                .chunk()
                .await
                .map_err(|_| Reason::DeliveryLost)?;
            let Some(chunk) = chunk else {
                return if self.buffered.is_empty() {
                    Ok(None)
                } else {
                    Err(Reason::DeliveryLost)
                };
            };
            self.consumed += chunk.len();
            if self.consumed > MAX_BODY {
                return Err(Reason::Contract);
            }
            self.buffered.extend_from_slice(&chunk);
        }
    }
}
