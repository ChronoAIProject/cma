//! Bound JSON topology before the V1 decoder allocates receipt/request trees.
use serde::de::{DeserializeSeed, MapAccess, SeqAccess, Visitor};

use super::{capacity_at_least, invalid};
use crate::failure::Result;

pub(crate) fn check(bytes: &[u8]) -> Result<()> {
    let mut nodes = 4096usize;
    let mut exceeded = false;
    let mut decoder = serde_json::Deserializer::from_slice(bytes);
    let result = Nodes {
        nodes: &mut nodes,
        exceeded: &mut exceeded,
    }
    .deserialize(&mut decoder);
    if exceeded {
        return Err(capacity_at_least("json_nodes", 4097));
    }
    result.map_err(|_| invalid())?;
    decoder.end().map_err(|_| invalid())
}

struct Nodes<'a> {
    nodes: &'a mut usize,
    exceeded: &'a mut bool,
}
impl Nodes<'_> {
    fn next(&mut self) -> Nodes<'_> {
        Nodes {
            nodes: self.nodes,
            exceeded: self.exceeded,
        }
    }
}

impl<'de> DeserializeSeed<'de> for Nodes<'_> {
    type Value = ();
    fn deserialize<D: serde::Deserializer<'de>>(
        self,
        decoder: D,
    ) -> std::result::Result<(), D::Error> {
        if *self.nodes == 0 {
            *self.exceeded = true;
            return Err(serde::de::Error::custom("inventory JSON budget"));
        }
        *self.nodes -= 1;
        decoder.deserialize_any(self)
    }
}

impl<'de> Visitor<'de> for Nodes<'_> {
    type Value = ();
    fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("bounded JSON")
    }
    fn visit_bool<E: serde::de::Error>(self, _: bool) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: serde::de::Error>(self, _: i64) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: serde::de::Error>(self, _: u64) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: serde::de::Error>(self, _: f64) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_str<E: serde::de::Error>(self, _: &str) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<(), E> {
        Ok(())
    }
    fn visit_seq<A: SeqAccess<'de>>(
        mut self,
        mut sequence: A,
    ) -> std::result::Result<(), A::Error> {
        while sequence.next_element_seed(self.next())?.is_some() {}
        Ok(())
    }
    fn visit_map<A: MapAccess<'de>>(mut self, mut map: A) -> std::result::Result<(), A::Error> {
        while map.next_key_seed(self.next())?.is_some() {
            map.next_value_seed(self.next())?;
        }
        Ok(())
    }
}
