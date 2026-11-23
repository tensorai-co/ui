//! `ui::Serde<T>`: send any `serde` type over IPC (the opt-in `serde` feature).
//!
//! ```ignore
//! #[derive(serde::Deserialize, serde::Serialize)]
//! struct ExportRequest { path: String, pages: Option<u32> }
//!
//! .invoke("export", |_ctx, ui::Serde(req): ui::Serde<ExportRequest>| ui::Serde(report(req)))
//! ```
//!
//! Values map directly between serde and the IPC `Value` tree, without a JSON
//! string in between; `serde_bytes`-style byte buffers stay binary.

use std::collections::BTreeMap;
use std::fmt;

use serde::de::{self, DeserializeOwned, IntoDeserializer, Visitor};
use serde::ser::{self, Serialize};

use crate::ipc::{DecodeError, FromIpc, IntoIpc, Value};

/// Wraps a `serde` type so it can be an IPC argument or result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Serde<T>(pub T);

impl<T: DeserializeOwned> FromIpc for Serde<T> {
    fn from_ipc(value: &Value) -> Result<Self, DecodeError> {
        T::deserialize(ValueDe(value))
            .map(Serde)
            .map_err(|e| DecodeError::new(e.0))
    }
}

impl<T: Serialize> IntoIpc for Serde<T> {
    fn into_ipc(self) -> Value {
        self.0
            .serialize(ValueSer)
            .unwrap_or_else(|e| Value::String(format!("serialize error: {}", e.0)))
    }
}

impl<T: Serialize> crate::ts::TsType for Serde<T> {
    fn ts_type() -> String {
        "unknown".into()
    }
}

/// Error for both directions.
#[derive(Debug)]
pub struct Error(String);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl de::Error for Error {
    fn custom<M: fmt::Display>(msg: M) -> Self {
        Self(msg.to_string())
    }
}

impl ser::Error for Error {
    fn custom<M: fmt::Display>(msg: M) -> Self {
        Self(msg.to_string())
    }
}

// ----------------------------------------------------------------- Deserialize

struct ValueDe<'a>(&'a Value);

impl<'de, 'a> de::Deserializer<'de> for ValueDe<'a> {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Error> {
        match self.0 {
            Value::Null => v.visit_unit(),
            Value::Bool(b) => v.visit_bool(*b),
            Value::Int(i) => v.visit_i64(*i),
            Value::Float(f) => v.visit_f64(*f),
            Value::String(s) => v.visit_str(s),
            Value::Bytes(b) => v.visit_bytes(b),
            Value::Array(items) => v.visit_seq(Seq(items.iter())),
            Value::Object(map) => v.visit_map(Map {
                iter: map.iter(),
                value: None,
            }),
        }
    }

    fn deserialize_option<V: Visitor<'de>>(self, v: V) -> Result<V::Value, Error> {
        match self.0 {
            Value::Null => v.visit_none(),
            _ => v.visit_some(self),
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        _name: &'static str,
        v: V,
    ) -> Result<V::Value, Error> {
        v.visit_newtype_struct(self)
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _name: &'static str,
        _variants: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, Error> {
        match self.0 {
            // "Unit"
            Value::String(s) => v.visit_enum(s.as_str().into_deserializer()),
            // { "Variant": content }
            Value::Object(map) if map.len() == 1 => {
                let (k, val) = map.iter().next().expect("one entry");
                v.visit_enum(Enum {
                    variant: k,
                    value: val,
                })
            }
            _ => Err(de::Error::custom(
                "expected a string or a one-key object for an enum",
            )),
        }
    }

    serde::forward_to_deserialize_any! {
        bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string
        bytes byte_buf unit unit_struct seq tuple tuple_struct map struct identifier ignored_any
    }
}

struct Seq<'a>(std::slice::Iter<'a, Value>);

impl<'de, 'a> de::SeqAccess<'de> for Seq<'a> {
    type Error = Error;
    fn next_element_seed<T: de::DeserializeSeed<'de>>(
        &mut self,
        seed: T,
    ) -> Result<Option<T::Value>, Error> {
        self.0
            .next()
            .map(|v| seed.deserialize(ValueDe(v)))
            .transpose()
    }
}

struct Map<'a> {
    iter: std::collections::btree_map::Iter<'a, String, Value>,
    value: Option<&'a Value>,
}

impl<'de, 'a> de::MapAccess<'de> for Map<'a> {
    type Error = Error;
    fn next_key_seed<K: de::DeserializeSeed<'de>>(
        &mut self,
        seed: K,
    ) -> Result<Option<K::Value>, Error> {
        match self.iter.next() {
            Some((k, v)) => {
                self.value = Some(v);
                seed.deserialize(k.as_str().into_deserializer()).map(Some)
            }
            None => Ok(None),
        }
    }
    fn next_value_seed<V: de::DeserializeSeed<'de>>(&mut self, seed: V) -> Result<V::Value, Error> {
        let v = self
            .value
            .take()
            .ok_or_else(|| de::Error::custom("value without key"))?;
        seed.deserialize(ValueDe(v))
    }
}

struct Enum<'a> {
    variant: &'a str,
    value: &'a Value,
}

impl<'de, 'a> de::EnumAccess<'de> for Enum<'a> {
    type Error = Error;
    type Variant = ValueDe<'a>;
    fn variant_seed<V: de::DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Self::Variant), Error> {
        let tag = seed.deserialize(self.variant.into_deserializer())?;
        Ok((tag, ValueDe(self.value)))
    }
}

impl<'de, 'a> de::VariantAccess<'de> for ValueDe<'a> {
    type Error = Error;
    fn unit_variant(self) -> Result<(), Error> {
        Ok(())
    }
    fn newtype_variant_seed<T: de::DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, Error> {
        seed.deserialize(self)
    }
    fn tuple_variant<V: Visitor<'de>>(self, _len: usize, v: V) -> Result<V::Value, Error> {
        de::Deserializer::deserialize_any(self, v)
    }
    fn struct_variant<V: Visitor<'de>>(
        self,
        _fields: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, Error> {
        de::Deserializer::deserialize_any(self, v)
    }
}

// ------------------------------------------------------------------- Serialize

struct ValueSer;

fn int<T: TryInto<i64>>(v: T) -> Result<Value, Error> {
    v.try_into()
        .map(Value::Int)
        .map_err(|_| Error("integer out of range for IPC".into()))
}

impl ser::Serializer for ValueSer {
    type Ok = Value;
    type Error = Error;
    type SerializeSeq = SeqSer;
    type SerializeTuple = SeqSer;
    type SerializeTupleStruct = SeqSer;
    type SerializeTupleVariant = VariantSeqSer;
    type SerializeMap = MapSer;
    type SerializeStruct = MapSer;
    type SerializeStructVariant = VariantMapSer;

    fn serialize_bool(self, v: bool) -> Result<Value, Error> {
        Ok(Value::Bool(v))
    }
    fn serialize_i8(self, v: i8) -> Result<Value, Error> {
        int(v)
    }
    fn serialize_i16(self, v: i16) -> Result<Value, Error> {
        int(v)
    }
    fn serialize_i32(self, v: i32) -> Result<Value, Error> {
        int(v)
    }
    fn serialize_i64(self, v: i64) -> Result<Value, Error> {
        int(v)
    }
    fn serialize_u8(self, v: u8) -> Result<Value, Error> {
        int(v)
    }
    fn serialize_u16(self, v: u16) -> Result<Value, Error> {
        int(v)
    }
    fn serialize_u32(self, v: u32) -> Result<Value, Error> {
        int(v)
    }
    fn serialize_u64(self, v: u64) -> Result<Value, Error> {
        int(v)
    }
    fn serialize_f32(self, v: f32) -> Result<Value, Error> {
        Ok(Value::Float(f64::from(v)))
    }
    fn serialize_f64(self, v: f64) -> Result<Value, Error> {
        Ok(Value::Float(v))
    }
    fn serialize_char(self, v: char) -> Result<Value, Error> {
        Ok(Value::String(v.to_string()))
    }
    fn serialize_str(self, v: &str) -> Result<Value, Error> {
        Ok(Value::String(v.to_owned()))
    }
    fn serialize_bytes(self, v: &[u8]) -> Result<Value, Error> {
        Ok(Value::Bytes(v.to_vec()))
    }
    fn serialize_none(self) -> Result<Value, Error> {
        Ok(Value::Null)
    }
    fn serialize_some<T: ?Sized + Serialize>(self, v: &T) -> Result<Value, Error> {
        v.serialize(self)
    }
    fn serialize_unit(self) -> Result<Value, Error> {
        Ok(Value::Null)
    }
    fn serialize_unit_struct(self, _name: &'static str) -> Result<Value, Error> {
        Ok(Value::Null)
    }
    fn serialize_unit_variant(
        self,
        _n: &'static str,
        _i: u32,
        variant: &'static str,
    ) -> Result<Value, Error> {
        Ok(Value::String(variant.to_owned()))
    }
    fn serialize_newtype_struct<T: ?Sized + Serialize>(
        self,
        _n: &'static str,
        v: &T,
    ) -> Result<Value, Error> {
        v.serialize(self)
    }
    fn serialize_newtype_variant<T: ?Sized + Serialize>(
        self,
        _n: &'static str,
        _i: u32,
        variant: &'static str,
        v: &T,
    ) -> Result<Value, Error> {
        Ok(Value::Object(BTreeMap::from([(
            variant.to_owned(),
            v.serialize(ValueSer)?,
        )])))
    }
    fn serialize_seq(self, len: Option<usize>) -> Result<SeqSer, Error> {
        Ok(SeqSer(Vec::with_capacity(len.unwrap_or(0))))
    }
    fn serialize_tuple(self, len: usize) -> Result<SeqSer, Error> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_struct(self, _n: &'static str, len: usize) -> Result<SeqSer, Error> {
        self.serialize_seq(Some(len))
    }
    fn serialize_tuple_variant(
        self,
        _n: &'static str,
        _i: u32,
        variant: &'static str,
        len: usize,
    ) -> Result<VariantSeqSer, Error> {
        Ok(VariantSeqSer(variant, Vec::with_capacity(len)))
    }
    fn serialize_map(self, _len: Option<usize>) -> Result<MapSer, Error> {
        Ok(MapSer(BTreeMap::new(), None))
    }
    fn serialize_struct(self, _n: &'static str, _len: usize) -> Result<MapSer, Error> {
        self.serialize_map(None)
    }
    fn serialize_struct_variant(
        self,
        _n: &'static str,
        _i: u32,
        variant: &'static str,
        _len: usize,
    ) -> Result<VariantMapSer, Error> {
        Ok(VariantMapSer(variant, BTreeMap::new()))
    }
}

struct SeqSer(Vec<Value>);

impl ser::SerializeSeq for SeqSer {
    type Ok = Value;
    type Error = Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Error> {
        self.0.push(v.serialize(ValueSer)?);
        Ok(())
    }
    fn end(self) -> Result<Value, Error> {
        Ok(Value::Array(self.0))
    }
}

impl ser::SerializeTuple for SeqSer {
    type Ok = Value;
    type Error = Error;
    fn serialize_element<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Error> {
        ser::SerializeSeq::serialize_element(self, v)
    }
    fn end(self) -> Result<Value, Error> {
        ser::SerializeSeq::end(self)
    }
}

impl ser::SerializeTupleStruct for SeqSer {
    type Ok = Value;
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Error> {
        ser::SerializeSeq::serialize_element(self, v)
    }
    fn end(self) -> Result<Value, Error> {
        ser::SerializeSeq::end(self)
    }
}

struct VariantSeqSer(&'static str, Vec<Value>);

impl ser::SerializeTupleVariant for VariantSeqSer {
    type Ok = Value;
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Error> {
        self.1.push(v.serialize(ValueSer)?);
        Ok(())
    }
    fn end(self) -> Result<Value, Error> {
        Ok(Value::Object(BTreeMap::from([(
            self.0.to_owned(),
            Value::Array(self.1),
        )])))
    }
}

struct MapSer(BTreeMap<String, Value>, Option<String>);

impl ser::SerializeMap for MapSer {
    type Ok = Value;
    type Error = Error;
    fn serialize_key<T: ?Sized + Serialize>(&mut self, key: &T) -> Result<(), Error> {
        match key.serialize(ValueSer)? {
            Value::String(s) => self.1 = Some(s),
            Value::Int(i) => self.1 = Some(i.to_string()),
            _ => return Err(Error("IPC object keys must be strings or integers".into())),
        }
        Ok(())
    }
    fn serialize_value<T: ?Sized + Serialize>(&mut self, v: &T) -> Result<(), Error> {
        let key = self
            .1
            .take()
            .ok_or_else(|| Error("value without key".into()))?;
        self.0.insert(key, v.serialize(ValueSer)?);
        Ok(())
    }
    fn end(self) -> Result<Value, Error> {
        Ok(Value::Object(self.0))
    }
}

impl ser::SerializeStruct for MapSer {
    type Ok = Value;
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        v: &T,
    ) -> Result<(), Error> {
        self.0.insert(key.to_owned(), v.serialize(ValueSer)?);
        Ok(())
    }
    fn end(self) -> Result<Value, Error> {
        Ok(Value::Object(self.0))
    }
}

struct VariantMapSer(&'static str, BTreeMap<String, Value>);

impl ser::SerializeStructVariant for VariantMapSer {
    type Ok = Value;
    type Error = Error;
    fn serialize_field<T: ?Sized + Serialize>(
        &mut self,
        key: &'static str,
        v: &T,
    ) -> Result<(), Error> {
        self.1.insert(key.to_owned(), v.serialize(ValueSer)?);
        Ok(())
    }
    fn end(self) -> Result<Value, Error> {
        Ok(Value::Object(BTreeMap::from([(
            self.0.to_owned(),
            Value::Object(self.1),
        )])))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    // serde's own impls for std types (no derive, so tests need no extra crates).
    type Req = (String, Option<u32>, Vec<String>, BTreeMap<String, f64>);

    #[test]
    fn round_trips_through_value() {
        let req: Req = (
            "a.pdf".into(),
            None,
            vec!["x".into(), "y".into()],
            BTreeMap::from([("scale".into(), 1.5)]),
        );
        let v = Serde(req.clone()).into_ipc();
        assert_eq!(
            v,
            Value::Array(vec![
                Value::String("a.pdf".into()),
                Value::Null,
                Value::Array(vec![Value::String("x".into()), Value::String("y".into())]),
                Value::Object(BTreeMap::from([("scale".into(), Value::Float(1.5))])),
            ])
        );
        let Serde(back): Serde<Req> = Serde::from_ipc(&v).unwrap();
        assert_eq!(back, req);

        // Page JSON: integers widen to floats, options accept values, errors surface.
        let page = crate::json!(["b", 4, [], { "scale": 2 }]);
        let Serde(r): Serde<Req> = Serde::from_ipc(&page).unwrap();
        assert_eq!((r.1, r.3["scale"]), (Some(4), 2.0));
        assert!(Serde::<Req>::from_ipc(&crate::json!([1, null, [], {}])).is_err());

        // Unit-variant style strings and binary data.
        let Serde(bytes): Serde<Vec<u8>> =
            Serde::from_ipc(&Value::Array(vec![Value::Int(7)])).unwrap();
        assert_eq!(bytes, [7]);
        assert_eq!(
            Serde(u64::MAX).into_ipc(),
            Value::String("serialize error: integer out of range for IPC".into())
        );
    }
}
