//! Validate JSON structure without constructing an owned Value tree.

use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};

const MAX_NODES: usize = 8192;
const MAX_DEPTH: usize = 64;
const MAX_BYTES: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Refusal {
    Bytes,
    Nodes,
    Depth,
    InvalidJson,
    PrivateKey,
}

impl Refusal {
    pub(super) fn adapter_error(self) -> crate::AdapterError {
        let reason = match self {
            Self::Bytes => "runtime output JSON exceeds its byte limit",
            Self::Nodes => "runtime output JSON exceeds its node limit",
            Self::Depth => "runtime output JSON exceeds its nesting limit",
            Self::InvalidJson => "runtime output JSON is malformed",
            Self::PrivateKey => "runtime output JSON contains a reserved parser key",
        };
        crate::AdapterError::Rejected(reason.into())
    }
}

pub(super) fn validate(payload: &[u8]) -> Result<(), Refusal> {
    inspect(payload).map(|_| ())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Shape {
    pub nodes: usize,
}

pub(super) fn inspect(payload: &[u8]) -> Result<Shape, Refusal> {
    inspect_with_limits(payload, MAX_BYTES, MAX_NODES, MAX_DEPTH)
}

#[cfg(test)]
fn validate_with_limits(
    payload: &[u8],
    max_bytes: usize,
    max_nodes: usize,
    max_depth: usize,
) -> Result<(), Refusal> {
    inspect_with_limits(payload, max_bytes, max_nodes, max_depth).map(|_| ())
}

fn inspect_with_limits(
    payload: &[u8],
    max_bytes: usize,
    max_nodes: usize,
    max_depth: usize,
) -> Result<Shape, Refusal> {
    if payload.len() > max_bytes {
        return Err(Refusal::Bytes);
    }
    let mut budget = Budget {
        remaining: max_nodes,
        max_depth,
        refusal: None,
    };
    let mut decoder = serde_json::Deserializer::from_slice(payload);
    let result = Node {
        budget: &mut budget,
        depth: 0,
    }
    .deserialize(&mut decoder);
    match result.and_then(|()| decoder.end()) {
        Ok(()) => Ok(Shape {
            nodes: max_nodes - budget.remaining,
        }),
        Err(_) => Err(budget.refusal.unwrap_or(Refusal::InvalidJson)),
    }
}

struct Budget {
    remaining: usize,
    max_depth: usize,
    refusal: Option<Refusal>,
}

impl Budget {
    fn refuse<E: de::Error>(&mut self, refusal: Refusal) -> E {
        self.refusal = Some(refusal);
        E::custom("runtime output JSON structure limit")
    }

    fn node<E: de::Error>(&mut self) -> Result<(), E> {
        if self.remaining == 0 {
            return Err(self.refuse(Refusal::Nodes));
        }
        self.remaining -= 1;
        Ok(())
    }

    fn container<E: de::Error>(&mut self, depth: usize) -> Result<(), E> {
        if depth > self.max_depth {
            return Err(self.refuse(Refusal::Depth));
        }
        Ok(())
    }
}

struct Node<'a> {
    budget: &'a mut Budget,
    depth: usize,
}

impl<'de> DeserializeSeed<'de> for Node<'_> {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(self, decoder: D) -> Result<(), D::Error> {
        self.budget.node()?;
        decoder.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Node<'_> {
    type Value = ();

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a bounded JSON value")
    }

    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: de::Error>(self, _: f64) -> Result<(), E> {
        Ok(())
    }
    fn visit_str<E: de::Error>(self, _: &str) -> Result<(), E> {
        Ok(())
    }

    fn visit_seq<A: SeqAccess<'de>>(self, mut sequence: A) -> Result<(), A::Error> {
        let depth = self.depth + 1;
        self.budget.container(depth)?;
        while sequence
            .next_element_seed(Node {
                budget: &mut *self.budget,
                depth,
            })?
            .is_some()
        {}
        Ok(())
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let depth = self.depth + 1;
        self.budget.container(depth)?;
        while map
            .next_key_seed(Key {
                budget: &mut *self.budget,
            })?
            .is_some()
        {
            map.next_value_seed(Node {
                budget: &mut *self.budget,
                depth,
            })?;
        }
        Ok(())
    }
}

struct Key<'a> {
    budget: &'a mut Budget,
}

impl<'de> DeserializeSeed<'de> for Key<'_> {
    type Value = ();

    fn deserialize<D: de::Deserializer<'de>>(self, decoder: D) -> Result<(), D::Error> {
        self.budget.node()?;
        decoder.deserialize_str(KeyVisitor {
            budget: self.budget,
        })
    }
}

struct KeyVisitor<'a> {
    budget: &'a mut Budget,
}
impl Visitor<'_> for KeyVisitor<'_> {
    type Value = ();
    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a JSON object key")
    }
    fn visit_str<E: de::Error>(self, key: &str) -> Result<(), E> {
        // Value's raw_value/arbitrary_precision deserializers interpret these
        // keys as nested parsers. Refuse before they can bypass this node count.
        if matches!(
            key,
            "$serde_json::private::RawValue" | "$serde_json::private::Number"
        ) {
            return Err(self.budget.refuse(Refusal::PrivateKey));
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "output_json_budget_tests.rs"]
mod tests;
