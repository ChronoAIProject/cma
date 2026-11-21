//! Socket timestamps carry no request headers, credentials, or content.
use crate::telemetry::timing::IngressTiming;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, OnceLock},
    time::Instant,
};

pub type Pending = Arc<Mutex<HashMap<u32, Arc<Observation>>>>;
pub type ActiveWrite = Arc<Mutex<Option<Arc<Stamp>>>>;

#[derive(Debug)]
pub struct Observation {
    pub at: OnceLock<Instant>,
    timing: Option<IngressTiming>,
}
impl Observation {
    pub fn new(timing: Option<IngressTiming>) -> Arc<Self> {
        Arc::new(Self {
            at: OnceLock::new(),
            timing,
        })
    }
    fn written(&self, at: Instant) {
        if self.at.set(at).is_ok() {
            if let Some(timing) = &self.timing {
                timing.written(at);
            }
        }
    }
}
#[derive(Default)]
pub struct Stamp(Mutex<(Option<Instant>, Option<Arc<Observation>>)>);
impl Stamp {
    pub fn written(&self, at: Instant) {
        let mut state = self.0.lock().unwrap();
        if state.0.is_none() {
            state.0 = Some(at);
            if let Some(observation) = state.1.take() {
                observation.written(at);
            }
        }
    }
    pub fn observe(&self, observation: Arc<Observation>) {
        let mut state = self.0.lock().unwrap();
        if let Some(at) = state.0 {
            observation.written(at);
        } else {
            state.1 = Some(observation);
        }
    }
}

/// Only the fixed HTTP/2 frame header is inspected. HPACK and data remain opaque.
pub struct Frames {
    preface: usize,
    header: [u8; 9],
    used: usize,
    payload: usize,
    first: Option<Arc<Stamp>>,
    pending: Pending,
    h2: bool,
}
impl Frames {
    pub fn new(h2: bool, pending: Pending) -> Self {
        Self {
            preface: if h2 { 24 } else { 0 },
            header: [0; 9],
            used: 0,
            payload: 0,
            first: None,
            pending,
            h2,
        }
    }
    pub fn limit(&self) -> usize {
        if !self.h2 {
            usize::MAX
        } else if self.preface > 0 {
            self.preface
        } else if self.payload > 0 {
            self.payload
        } else {
            9 - self.used
        }
    }
    pub fn accepted(&mut self, bytes: &[u8], stamp: Arc<Stamp>) {
        if bytes.is_empty() {
            return;
        }
        if !self.h2 {
            if let Some(observation) = self.pending.lock().unwrap().remove(&0) {
                stamp.observe(observation);
            }
        } else if self.preface > 0 {
            self.preface -= bytes.len();
        } else if self.payload > 0 {
            self.payload -= bytes.len();
        } else {
            if self.used == 0 {
                self.first = Some(stamp);
            }
            self.header[self.used..self.used + bytes.len()].copy_from_slice(bytes);
            self.used += bytes.len();
            if self.used == 9 {
                self.payload =
                    u32::from_be_bytes([0, self.header[0], self.header[1], self.header[2]])
                        as usize;
                let stream = u32::from_be_bytes(self.header[5..9].try_into().unwrap()) & 0x7fffffff;
                if self.header[3] == 1 {
                    if let Some(observation) = self.pending.lock().unwrap().remove(&stream) {
                        self.first.as_ref().unwrap().observe(observation);
                    }
                }
                self.used = 0;
                self.first = None;
            }
        }
    }
}
