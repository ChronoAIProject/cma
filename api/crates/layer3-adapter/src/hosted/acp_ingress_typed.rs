//! Borrowed sizing for the pinned schema's owned containers and decode scratch.

use crate::hosted::runtime_decode;
use crate::ingress_budget::{
    sizing::shared_owner_bytes, AllocationPlan, IngressClass, IngressOwnershipPolicy,
    IngressWorkScope, OwnedWork, ReservationBundle,
};
use agent_client_protocol_schema::v1 as schema;
use serde_json::Value;

const MAX_NODES: usize = 8192;
const MAX_DEPTH: usize = 64;

pub(super) fn maximum_working_bytes() -> crate::Result<usize> {
    // Each sequence element contributes one node, each map pair contributes
    // two. Thus minimum-capacity/growth slots total at most four per node,
    // even for many tiny containers. Strings cannot exceed the wire bound.
    let maximum = Shape {
        nodes: MAX_NODES,
        string_bytes: super::super::MAX_FRAME_BYTES,
        sequence_slots: 4 * MAX_NODES,
        map_entries: 0,
        buffered_slots: 4 * MAX_NODES,
    };
    // Credit-error scanning and typed result decoding are sequential branches
    // of turn_outcome_with_policy; their scratch allocations never coexist.
    Ok(maximum
        .working()?
        .max(3 * super::super::MAX_FRAME_BYTES + 1024))
}

#[cfg(test)]
pub(super) fn maximum_retained_bytes() -> crate::Result<usize> {
    // A map pair consumes two nodes, while an array element consumes one.
    // Taking the larger coefficient avoids charging both maxima to one node.
    let map_node = 11 * size_of::<(String, Value)>() + 14 * size_of::<usize>();
    let per_node = (4 * largest_element()).max(map_node.div_ceil(2));
    AllocationPlan::new(
        "maximum typed retained output",
        &[
            super::super::MAX_FRAME_BYTES,
            MAX_NODES * (per_node + 3 * size_of::<usize>()),
            shared_owner_bytes::<crate::ConversationUpdate>() * 2,
        ],
    )
    .map(|plan| plan.requested_bytes())
}

#[derive(Default)]
pub(in crate::hosted) struct Shape {
    pub(super) nodes: usize,
    pub(super) string_bytes: usize,
    sequence_slots: usize,
    map_entries: usize,
    buffered_slots: usize,
}

impl Shape {
    pub(in crate::hosted) fn serialized(value: &impl serde::Serialize) -> crate::Result<Self> {
        let mut counter = SerializedShape {
            shape: Self::default(),
            depth: 0,
        };
        value.serialize(&mut counter).map_err(|_| invalid())?;
        Ok(counter.shape)
    }

    pub(in crate::hosted) fn inspect(value: &Value) -> crate::Result<Self> {
        let mut shape = Self::default();
        shape.visit(value, 0)?;
        Ok(shape)
    }

    fn visit(&mut self, value: &Value, depth: usize) -> crate::Result<()> {
        self.nodes = add(self.nodes, 1)?;
        if self.nodes > MAX_NODES || depth > MAX_DEPTH {
            return Err(invalid());
        }
        match value {
            Value::String(value) => self.string_bytes = add(self.string_bytes, value.len())?,
            Value::Array(values) => {
                if !values.is_empty() {
                    // Vec's initial four slots and doubling can coexist with
                    // the old allocation while reallocating. Retained capacity
                    // remains covered even for tolerated/discarded elements.
                    self.sequence_slots = add(self.sequence_slots, mul(values.len(), 2)?.max(4))?;
                    self.buffered_slots = add(self.buffered_slots, mul(values.len(), 3)?.max(4))?;
                }
                for value in values {
                    self.visit(value, depth + 1)?;
                }
            }
            Value::Object(values) => {
                self.map_entries = add(self.map_entries, values.len())?;
                if !values.is_empty() {
                    self.buffered_slots =
                        add(self.buffered_slots, mul(mul(values.len(), 3)?.max(4), 2)?)?;
                }
                for (key, value) in values {
                    self.nodes = add(self.nodes, 1)?;
                    self.string_bytes = add(self.string_bytes, key.len())?;
                    self.visit(value, depth + 1)?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    pub(in crate::hosted) fn retained<T>(&self) -> crate::Result<usize> {
        // A BTree node has at most eleven key/value pairs and twelve edges.
        // Charging one whole node per input entry also covers sparse maps and
        // internal nodes; keys and string payloads are counted separately.
        let map_node = 11 * size_of::<(String, Value)>() + 14 * size_of::<usize>();
        AllocationPlan::new(
            "typed retained layout",
            &[
                self.string_bytes,
                mul(self.map_entries, map_node)?,
                mul(self.sequence_slots, largest_element())?,
                mul(self.nodes, 3 * size_of::<usize>())?, // Arc<str> headers/alignment.
                shared_owner_bytes::<T>(),
                shared_owner_bytes::<crate::ConversationUpdate>(),
            ],
        )
        .map(|plan| plan.requested_bytes())
    }

    fn working(&self) -> crate::Result<usize> {
        // Serde's Content variants are at most a Vec/String plus discriminant
        // and scalar alignment. Schema 1.11's longest buffering path is:
        // update tag, config DefaultOnError, VecSkipError, flattened kind,
        // kind tag, options union, grouped DefaultOnError, VecSkipError,
        // and option metadata DefaultOnError. Each of these nine layers may
        // retain Content vectors. buffered_slots covers old plus doubled new
        // capacity, the initial four slots, and both members of map pairs.
        // Union candidates run sequentially; typed output/Arc conversions can
        // overlap a candidate and therefore have their own allowances below.
        let content = size_of::<Value>().max(size_of::<String>()) + 2 * size_of::<usize>();
        AllocationPlan::new(
            "typed decode overlap",
            &[
                mul(self.buffered_slots, 9 * content)?,
                mul(self.string_bytes, 2)?,
                mul(self.sequence_slots, largest_element())?,
            ],
        )
        .map(|plan| plan.requested_bytes())
    }
}

fn largest_element() -> usize {
    // Every owned sequence element reachable from SessionUpdate, permissions
    // and prompt results in schema 1.11. Root inline storage is charged separately.
    [
        size_of::<schema::ContentBlock>(),
        size_of::<schema::ToolCallContent>(),
        size_of::<schema::ToolCallLocation>(),
        size_of::<schema::PermissionOption>(),
        size_of::<schema::PlanEntry>(),
        size_of::<schema::AvailableCommand>(),
        size_of::<schema::SessionConfigOption>(),
        size_of::<schema::SessionConfigSelectOption>(),
        size_of::<schema::SessionConfigSelectGroup>(),
        size_of::<schema::Role>(),
        size_of::<Value>(),
    ]
    .into_iter()
    .max()
    .expect("schema element layouts are nonempty")
}

pub(in crate::hosted) struct TypedAdmission {
    retained: ReservationBundle,
    _working: ReservationBundle,
}

impl TypedAdmission {
    pub(in crate::hosted) fn new<T>(
        value: &Value,
        policy: &IngressOwnershipPolicy,
    ) -> crate::Result<Self> {
        Self::with_alternative_work::<T>(value, policy, 0, 0)
    }

    pub(in crate::hosted) fn with_alternative_work<T>(
        value: &Value,
        policy: &IngressOwnershipPolicy,
        retained_extra: usize,
        alternative_working: usize,
    ) -> crate::Result<Self> {
        let shape = Shape::inspect(value)?;
        let retained = policy.reserve(
            IngressClass::Data,
            AllocationPlan::new("typed output", &[shape.retained::<T>()?, retained_extra])?,
        )?;
        let working = policy.reserve(
            IngressClass::Transformation,
            AllocationPlan::new(
                "typed decode or diagnostic scan",
                &[shape.working()?.max(alternative_working)],
            )?,
        )?;
        Ok(Self {
            retained,
            _working: working,
        })
    }

    pub(in crate::hosted) fn initialize<T>(self, initialize: impl FnOnce() -> T) -> OwnedWork<T> {
        IngressWorkScope::new(self.retained).initialize_owned(initialize)
    }

    pub(in crate::hosted) fn decode<T: serde::de::DeserializeOwned>(
        self,
        value: &Value,
    ) -> Result<OwnedWork<T>, OwnedWork<runtime_decode::Error>> {
        self.initialize(|| runtime_decode::decode(value, true))
            .transpose()
    }
}

fn add(a: usize, b: usize) -> crate::Result<usize> {
    a.checked_add(b).ok_or_else(invalid)
}
fn mul(a: usize, b: usize) -> crate::Result<usize> {
    a.checked_mul(b).ok_or_else(invalid)
}
fn invalid() -> crate::AdapterError {
    crate::AdapterError::Rejected("typed ingress layout exceeds supported bounds".into())
}

// Run the same Serialize implementation as to_value without creating its tree.
// Four slots per sequence item bound minimum Vec capacity and old/new growth;
// map pairs need two Content slots each. This also admits JSON built from typed
// input before a Value exists for the borrowed preflight above.
struct SerializedShape {
    shape: Shape,
    depth: usize,
}

#[derive(Debug)]
struct SizingError;
impl std::fmt::Display for SizingError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.write_str("serialized ingress exceeds its layout bound")
    }
}
impl std::error::Error for SizingError {}
impl serde::ser::Error for SizingError {
    fn custom<T: std::fmt::Display>(_: T) -> Self {
        Self
    }
}
type SizedResult<T> = Result<T, SizingError>;

impl SerializedShape {
    fn node(&mut self) -> SizedResult<()> {
        self.shape.nodes += 1;
        if self.shape.nodes > MAX_NODES || self.depth > MAX_DEPTH {
            return Err(SizingError);
        }
        Ok(())
    }
    fn string(&mut self, bytes: usize) -> SizedResult<()> {
        self.node()?;
        self.shape.string_bytes = self
            .shape
            .string_bytes
            .checked_add(bytes)
            .ok_or(SizingError)?;
        if self.shape.string_bytes > super::super::MAX_FRAME_BYTES {
            return Err(SizingError);
        }
        Ok(())
    }
    fn container(&mut self) -> SizedResult<SerializedContainer<'_>> {
        self.node()?;
        self.depth += 1;
        Ok(SerializedContainer {
            counter: self,
            levels: 1,
        })
    }
    fn variant(&mut self, name: &str) -> SizedResult<SerializedContainer<'_>> {
        let mut map = self.container()?;
        map.key(name)?;
        map.counter.node()?;
        map.counter.depth += 1;
        map.levels += 1;
        Ok(map)
    }
}

struct SerializedContainer<'a> {
    counter: &'a mut SerializedShape,
    levels: usize,
}
impl SerializedContainer<'_> {
    fn item<T: serde::Serialize + ?Sized>(&mut self, value: &T) -> SizedResult<()> {
        self.counter.shape.sequence_slots += 4;
        self.counter.shape.buffered_slots += 4;
        value.serialize(&mut *self.counter)
    }
    fn key<T: serde::Serialize + ?Sized>(&mut self, key: &T) -> SizedResult<()> {
        self.counter.shape.map_entries += 1;
        self.counter.shape.buffered_slots += 8;
        key.serialize(&mut *self.counter)
    }
    fn field<T: serde::Serialize + ?Sized>(&mut self, key: &str, value: &T) -> SizedResult<()> {
        self.key(key)?;
        value.serialize(&mut *self.counter)
    }
    fn finish(self) -> SizedResult<()> {
        self.counter.depth -= self.levels;
        Ok(())
    }
}

macro_rules! count_scalar {
    ($($method:ident: $ty:ty),* $(,)?) => {$(
        fn $method(self, _: $ty) -> SizedResult<()> {
            // Numeric/bool map keys are formatted as short owned strings by
            // serde_json. Counting this for scalar values too is conservative.
            self.string(40)
        }
    )*};
}
impl<'a> serde::Serializer for &'a mut SerializedShape {
    type Ok = ();
    type Error = SizingError;
    type SerializeSeq = SerializedContainer<'a>;
    type SerializeTuple = SerializedContainer<'a>;
    type SerializeTupleStruct = SerializedContainer<'a>;
    type SerializeTupleVariant = SerializedContainer<'a>;
    type SerializeMap = SerializedContainer<'a>;
    type SerializeStruct = SerializedContainer<'a>;
    type SerializeStructVariant = SerializedContainer<'a>;
    count_scalar!(serialize_bool: bool, serialize_i8: i8, serialize_i16: i16,
        serialize_i32: i32, serialize_i64: i64, serialize_i128: i128,
        serialize_u8: u8, serialize_u16: u16, serialize_u32: u32,
        serialize_u64: u64, serialize_u128: u128, serialize_f32: f32, serialize_f64: f64);
    fn serialize_char(self, value: char) -> SizedResult<()> {
        self.string(value.len_utf8())
    }
    fn serialize_str(self, value: &str) -> SizedResult<()> {
        self.string(value.len())
    }
    fn serialize_bytes(self, value: &[u8]) -> SizedResult<()> {
        use serde::ser::SerializeSeq;
        let mut sequence = self.serialize_seq(Some(value.len()))?;
        for byte in value {
            sequence.serialize_element(byte)?;
        }
        sequence.end()
    }
    fn serialize_none(self) -> SizedResult<()> {
        self.node()
    }
    fn serialize_some<T: serde::Serialize + ?Sized>(self, value: &T) -> SizedResult<()> {
        value.serialize(self)
    }
    fn serialize_unit(self) -> SizedResult<()> {
        self.node()
    }
    fn serialize_unit_struct(self, _: &'static str) -> SizedResult<()> {
        self.node()
    }
    fn serialize_unit_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
    ) -> SizedResult<()> {
        self.string(variant.len())
    }
    fn serialize_newtype_struct<T: serde::Serialize + ?Sized>(
        self,
        _: &'static str,
        value: &T,
    ) -> SizedResult<()> {
        value.serialize(self)
    }
    fn serialize_newtype_variant<T: serde::Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        value: &T,
    ) -> SizedResult<()> {
        let mut map = self.container()?;
        map.field(variant, value)?;
        map.finish()
    }
    fn serialize_seq(self, _: Option<usize>) -> SizedResult<Self::SerializeSeq> {
        self.container()
    }
    fn serialize_tuple(self, _: usize) -> SizedResult<Self::SerializeTuple> {
        self.container()
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> SizedResult<Self::SerializeTupleStruct> {
        self.container()
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> SizedResult<Self::SerializeTupleVariant> {
        self.variant(variant)
    }
    fn serialize_map(self, _: Option<usize>) -> SizedResult<Self::SerializeMap> {
        self.container()
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> SizedResult<Self::SerializeStruct> {
        self.container()
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        variant: &'static str,
        _: usize,
    ) -> SizedResult<Self::SerializeStructVariant> {
        self.variant(variant)
    }
    fn collect_str<T: std::fmt::Display + ?Sized>(self, value: &T) -> SizedResult<()> {
        struct Length(usize);
        impl std::fmt::Write for Length {
            fn write_str(&mut self, text: &str) -> std::fmt::Result {
                self.0 = self.0.checked_add(text.len()).ok_or(std::fmt::Error)?;
                Ok(())
            }
        }
        use std::fmt::Write;
        let mut length = Length(0);
        write!(&mut length, "{value}").map_err(|_| SizingError)?;
        // collect_str can grow a String before transferring it into Value.
        self.string(length.0.checked_mul(3).ok_or(SizingError)?)
    }
}
macro_rules! count_sequence {
    ($trait:ident, $method:ident) => {
        impl serde::ser::$trait for SerializedContainer<'_> {
            type Ok = ();
            type Error = SizingError;
            fn $method<T: serde::Serialize + ?Sized>(&mut self, value: &T) -> SizedResult<()> {
                self.item(value)
            }
            fn end(self) -> SizedResult<()> {
                self.finish()
            }
        }
    };
}
count_sequence!(SerializeSeq, serialize_element);
count_sequence!(SerializeTuple, serialize_element);
count_sequence!(SerializeTupleStruct, serialize_field);
count_sequence!(SerializeTupleVariant, serialize_field);
impl serde::ser::SerializeMap for SerializedContainer<'_> {
    type Ok = ();
    type Error = SizingError;
    fn serialize_key<T: serde::Serialize + ?Sized>(&mut self, key: &T) -> SizedResult<()> {
        self.key(key)
    }
    fn serialize_value<T: serde::Serialize + ?Sized>(&mut self, value: &T) -> SizedResult<()> {
        value.serialize(&mut *self.counter)
    }
    fn end(self) -> SizedResult<()> {
        self.finish()
    }
}
macro_rules! count_fields {
    ($trait:ident) => {
        impl serde::ser::$trait for SerializedContainer<'_> {
            type Ok = ();
            type Error = SizingError;
            fn serialize_field<T: serde::Serialize + ?Sized>(
                &mut self,
                key: &'static str,
                value: &T,
            ) -> SizedResult<()> {
                self.field(key, value)
            }
            fn end(self) -> SizedResult<()> {
                self.finish()
            }
        }
    };
}
count_fields!(SerializeStruct);
count_fields!(SerializeStructVariant);
