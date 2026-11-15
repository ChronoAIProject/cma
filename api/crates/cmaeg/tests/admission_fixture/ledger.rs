//! Synthetic provider custody and sink effects are independent of gateway state.
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs::File, io::Write, path::Path};

pub fn hash(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

#[derive(Clone, Serialize)]
pub struct ResponseRecord {
    pub response_id: String,
    pub event_id: String,
    pub body_sha256: String,
    pub conversation: String,
    pub metadata: Value,
    pub post_attempts: u64,
    pub sequence: u64,
}
#[derive(Clone, Serialize)]
pub struct Effect {
    pub event_sha256: String,
    pub body_sha256: String,
    pub text_matches: bool,
    pub sequence: u64,
    pub after_close: bool,
    pub receipt_sha256: String,
}
#[derive(Default)]
pub struct Ledger {
    pub responses: BTreeMap<String, ResponseRecord>,
    pub effects: Vec<Effect>,
    pub rejected_requests: u64,
    pub closed: bool,
    pub poisoned: bool,
    next_sequence: u64,
    journal: Option<File>,
    journal_sequence: u64,
    journal_hash: String,
}
impl Ledger {
    pub fn open(path: &Path) -> Result<Self, &'static str> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut ledger = Self {
            journal: Some(options.open(path).map_err(|_| "ledger creation")?),
            ..Self::default()
        };
        ledger.persist("created")?;
        Ok(ledger)
    }
    fn persist(&mut self, action: &str) -> Result<(), &'static str> {
        if self.poisoned {
            return Err("ledger unavailable");
        }
        let content = serde_json::json!({"sequence":self.journal_sequence+1,"previous_sha256":self.journal_hash,
            "action":action,"responses":self.responses,"effects":self.effects,"closed":self.closed,
            "rejected_requests":self.rejected_requests});
        let mut bytes = serde_json::to_vec(&content).map_err(|_| "ledger encoding")?;
        let digest = hash(&bytes);
        bytes.push(b'\n');
        if let Some(journal) = &mut self.journal {
            if journal
                .write_all(&bytes)
                .and_then(|()| journal.sync_all())
                .is_err()
            {
                self.poisoned = true;
                return Err("ledger persistence");
            }
        } else if !cfg!(test) {
            self.poisoned = true;
            return Err("ledger missing");
        }
        self.journal_sequence += 1;
        self.journal_hash = digest;
        Ok(())
    }
    pub fn close(&mut self) -> Result<(), &'static str> {
        self.closed = true;
        self.persist("closed")
    }
    pub fn reject<T>(&mut self, reason: &'static str) -> Result<T, &'static str> {
        self.rejected_requests += 1;
        self.persist("rejected")?;
        Err(reason)
    }
    pub fn admit(
        &mut self,
        key: &str,
        event: &str,
        body: &[u8],
        value: &Value,
    ) -> Result<ResponseRecord, &'static str> {
        if self.closed || self.poisoned {
            return self.reject("closed");
        }
        if key.is_empty() || key.len() > 512 {
            return self.reject("idempotency");
        }
        let body_sha256 = hash(body);
        if let Some(existing) = self.responses.get_mut(key) {
            existing.post_attempts += 1;
            if existing.body_sha256 != body_sha256 || existing.event_id != event {
                return self.reject("conflict");
            }
            let replay = existing.clone();
            self.persist("provider_replay")?;
            return Ok(replay);
        }
        let conversation = value["conversation"]
            .as_str()
            .filter(|s| !s.is_empty() && s.len() <= 512)
            .ok_or("conversation")?;
        if !value["metadata"].is_object() {
            return Err("metadata");
        }
        self.next_sequence += 1;
        let record = ResponseRecord {
            response_id: format!("resp_{}", uuid::Uuid::new_v4().simple()),
            event_id: event.into(),
            body_sha256,
            conversation: conversation.into(),
            metadata: value["metadata"].clone(),
            post_attempts: 1,
            sequence: self.next_sequence,
        };
        self.responses.insert(key.into(), record.clone());
        self.persist("provider_admitted")?;
        Ok(record)
    }
    pub fn effect(&mut self, event: &str, body: &[u8], text: &str) -> Result<Value, &'static str> {
        if self.poisoned {
            return Err("ledger unavailable");
        }
        if self.effects.len() >= 512 {
            return self.reject("sink effect limit");
        }
        self.next_sequence += 1;
        let receipt = serde_json::json!({"message_id":format!("reply-{}", self.next_sequence),"platform_message_id":format!("platform-{}", self.next_sequence)});
        // Deliberately never deduplicate the sink: any duplicate send is evidence.
        self.effects.push(Effect {
            event_sha256: hash(event.as_bytes()),
            body_sha256: hash(body),
            text_matches: text == "fixture terminal text",
            sequence: self.next_sequence,
            after_close: self.closed,
            receipt_sha256: hash(receipt.to_string().as_bytes()),
        });
        self.persist("sink_effect")?;
        Ok(receipt)
    }
    pub fn snapshot(&self) -> Value {
        serde_json::json!({"closed":self.closed,"poisoned":self.poisoned,"journal_sequence":self.journal_sequence,
            "journal_sha256":self.journal_hash,"journal_attached":self.journal.is_some(),"rejected_requests":self.rejected_requests,
            "provider_operations":self.responses.iter().map(|(key,row)| serde_json::json!({"idempotency_sha256":hash(key.as_bytes()),
                "event_sha256":hash(row.event_id.as_bytes()),"response_sha256":hash(row.response_id.as_bytes()),"body_sha256":row.body_sha256,
                "post_attempts":row.post_attempts,"sequence":row.sequence})).collect::<Vec<_>>(),"sink_effects":self.effects})
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn provider_replays_exact_bytes_but_sink_retains_every_duplicate() {
        let mut ledger = Ledger::default();
        let body = serde_json::json!({"conversation":"synthetic","metadata":{}});
        let bytes = body.to_string().into_bytes();
        let first = ledger.admit("key", "event", &bytes, &body).unwrap();
        assert_eq!(
            ledger
                .admit("key", "event", &bytes, &body)
                .unwrap()
                .response_id,
            first.response_id
        );
        assert!(ledger.admit("key", "event", b"changed", &body).is_err());
        assert!(ledger.admit("key", "other", &bytes, &body).is_err());
        ledger
            .effect("event", b"body", "fixture terminal text")
            .unwrap();
        ledger
            .effect("event", b"body", "fixture terminal text")
            .unwrap();
        assert_eq!(ledger.responses.len(), 1);
        assert_eq!(ledger.effects.len(), 2);
        ledger.close().unwrap();
        assert!(ledger.admit("next", "event", &bytes, &body).is_err());
        ledger.effect("event", b"body", "wrong").unwrap();
        assert!(ledger.effects[2].after_close);
        assert!(!ledger.effects[2].text_matches);
        assert!(!ledger.snapshot().to_string().contains("synthetic"));
    }
    #[test]
    fn failed_journal_write_poisoning_prevents_acknowledgement_and_resend() {
        let path = std::env::temp_dir().join(format!("eg-ledger-failure-{}", uuid::Uuid::new_v4()));
        let mut ledger = Ledger::open(&path).unwrap();
        ledger.journal = Some(std::fs::File::open(&path).unwrap()); // read-only descriptor
        assert!(ledger
            .effect("event", b"reply", "fixture terminal text")
            .is_err());
        assert!(ledger.poisoned);
        let before = ledger.effects.len();
        assert!(ledger
            .effect("event", b"retry", "fixture terminal text")
            .is_err());
        assert_eq!(ledger.effects.len(), before);
        assert!(ledger
            .admit("key", "event", b"{}", &serde_json::json!({}))
            .is_err());
        assert!(ledger.close().is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap().lines().count(), 1);
        drop(ledger);
        std::fs::remove_file(path).unwrap();
    }
}
