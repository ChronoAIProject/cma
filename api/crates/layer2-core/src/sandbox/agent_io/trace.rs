use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Additive metadata returned after a response binds to a committed turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NyxidTrace {
    pub version: u16,
    pub turn_id: String,
    pub trace_id: String,
    pub parent_span_id: String,
    pub ingress_request_id: String,
}

impl NyxidTrace {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.version != 1 {
            return Err("unsupported trace version");
        }
        let turn = Uuid::parse_str(&self.turn_id).map_err(|_| "invalid turn id")?;
        let ingress =
            Uuid::parse_str(&self.ingress_request_id).map_err(|_| "invalid ingress request id")?;
        if turn.get_version_num() != 4 || ingress.get_version_num() != 4 {
            return Err("trace UUIDs must be version 4");
        }
        if self.trace_id != turn.simple().to_string()
            || self.trace_id.bytes().all(|byte| byte == b'0')
            || self.trace_id.bytes().any(|byte| byte.is_ascii_uppercase())
        {
            return Err("trace id does not match turn id");
        }
        if self.parent_span_id.len() != 16
            || !self
                .parent_span_id
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
            || self.parent_span_id.bytes().all(|byte| byte == b'0')
            || self
                .parent_span_id
                .bytes()
                .any(|byte| byte.is_ascii_uppercase())
        {
            return Err("invalid parent span id");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::NyxidTrace;

    fn valid() -> NyxidTrace {
        NyxidTrace {
            version: 1,
            turn_id: "11111111-1111-4111-8111-111111111111".into(),
            trace_id: "11111111111141118111111111111111".into(),
            parent_span_id: "0123456789abcdef".into(),
            ingress_request_id: "22222222-2222-4222-8222-222222222222".into(),
        }
    }

    #[test]
    fn validates_canonical_uuid_and_span_association() {
        assert!(valid().validate().is_ok());
        let mut changed = valid();
        changed.trace_id.replace_range(0..1, "2");
        assert!(changed.validate().is_err());
    }

    #[test]
    fn rejects_non_v4_or_zero_span_values() {
        let mut changed = valid();
        changed.turn_id = "11111111-1111-3111-8111-111111111111".into();
        assert!(changed.validate().is_err());
        let mut changed = valid();
        changed.parent_span_id = "0000000000000000".into();
        assert!(changed.validate().is_err());
    }
}
