//! Typed decode errors are fixed before formatting, including serde's default
//! unknown-field/variant/invalid-value helpers. No input-bearing diagnostic is
//! allocated and then merely hidden by an outer wrapper.
use serde::de::{self, DeserializeSeed, EnumAccess, MapAccess, SeqAccess, VariantAccess, Visitor};
use std::fmt;
#[derive(Debug)]
pub(super) struct FixedError;
impl fmt::Display for FixedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid witness")
    }
}
impl std::error::Error for FixedError {}
impl de::Error for FixedError {
    fn custom<T: fmt::Display>(_: T) -> Self {
        Self
    }
}
pub(super) struct Fixed<D>(pub D);
// Internally tagged derives buffer payloads in serde's Content and restart with
// ContentDeserializer, bypassing the outer Fixed wrapper. Re-enter at EVERY
// payload field; FixedSeed then keeps all nested structs/enums strict as well.
pub(super) fn payload<'de, D: de::Deserializer<'de>, T: serde::Deserialize<'de>>(
    d: D,
) -> Result<T, D::Error> {
    T::deserialize(Fixed(d)).map_err(bridge)
}
struct FixedVisitor<V>(V);
struct FixedSeed<S>(S);
// Empty bridges prevent cumulative UTF-8 error text generation at recursive
// parser boundaries. The public API returns its separate fixed error code.
fn bridge<E: de::Error>(_: FixedError) -> E {
    E::custom("")
}
impl<'de, S: DeserializeSeed<'de>> DeserializeSeed<'de> for FixedSeed<S> {
    type Value = S::Value;
    fn deserialize<D: de::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        self.0.deserialize(Fixed(d)).map_err(bridge)
    }
}
macro_rules! visit_scalar {($($name:ident:$ty:ty),*)=>{$(
 fn $name<E:de::Error>(self,v:$ty)->Result<Self::Value,E>{self.0.$name::<FixedError>(v).map_err(bridge)}
)*};}
impl<'de, V: Visitor<'de>> Visitor<'de> for FixedVisitor<V> {
    type Value = V::Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("witness value")
    }
    visit_scalar!(visit_bool:bool,visit_i8:i8,visit_i16:i16,visit_i32:i32,visit_i64:i64,visit_i128:i128,visit_u8:u8,visit_u16:u16,visit_u32:u32,visit_u64:u64,visit_u128:u128,visit_f32:f32,visit_f64:f64,visit_char:char,visit_str:&str,visit_string:String,visit_bytes:&[u8],visit_byte_buf:Vec<u8>);
    fn visit_borrowed_str<E: de::Error>(self, v: &'de str) -> Result<Self::Value, E> {
        self.0.visit_borrowed_str::<FixedError>(v).map_err(bridge)
    }
    fn visit_borrowed_bytes<E: de::Error>(self, v: &'de [u8]) -> Result<Self::Value, E> {
        self.0.visit_borrowed_bytes::<FixedError>(v).map_err(bridge)
    }
    fn visit_none<E: de::Error>(self) -> Result<Self::Value, E> {
        self.0.visit_none::<FixedError>().map_err(bridge)
    }
    fn visit_unit<E: de::Error>(self) -> Result<Self::Value, E> {
        self.0.visit_unit::<FixedError>().map_err(bridge)
    }
    fn visit_some<D: de::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        self.0.visit_some(Fixed(d)).map_err(bridge)
    }
    fn visit_newtype_struct<D: de::Deserializer<'de>>(self, d: D) -> Result<Self::Value, D::Error> {
        self.0.visit_newtype_struct(Fixed(d)).map_err(bridge)
    }
    fn visit_seq<A: SeqAccess<'de>>(self, a: A) -> Result<Self::Value, A::Error> {
        self.0.visit_seq(Fixed(a)).map_err(bridge)
    }
    fn visit_map<A: MapAccess<'de>>(self, a: A) -> Result<Self::Value, A::Error> {
        self.0.visit_map(Fixed(a)).map_err(bridge)
    }
    fn visit_enum<A: EnumAccess<'de>>(self, a: A) -> Result<Self::Value, A::Error> {
        self.0.visit_enum(Fixed(a)).map_err(bridge)
    }
}
impl<'de, D: de::Deserializer<'de>> de::Deserializer<'de> for Fixed<D> {
    type Error = FixedError;
    fn deserialize_any<V: Visitor<'de>>(self, v: V) -> Result<V::Value, FixedError> {
        self.0
            .deserialize_any(FixedVisitor(v))
            .map_err(|_| FixedError)
    }
    fn deserialize_option<V: Visitor<'de>>(self, v: V) -> Result<V::Value, FixedError> {
        self.0
            .deserialize_option(FixedVisitor(v))
            .map_err(|_| FixedError)
    }
    fn deserialize_enum<V: Visitor<'de>>(
        self,
        _: &'static str,
        _: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, FixedError> {
        self.0
            .deserialize_any(FixedVisitor(StringEnumOnly(v)))
            .map_err(|_| FixedError)
    }
    fn deserialize_identifier<V: Visitor<'de>>(self, v: V) -> Result<V::Value, FixedError> {
        self.0
            .deserialize_any(FixedVisitor(StringOnly(v)))
            .map_err(|_| FixedError)
    }
    fn deserialize_struct<V: Visitor<'de>>(
        self,
        _: &'static str,
        _: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, FixedError> {
        self.0
            .deserialize_any(FixedVisitor(MapOnly(v)))
            .map_err(|_| FixedError)
    }
    fn deserialize_map<V: Visitor<'de>>(self, v: V) -> Result<V::Value, FixedError> {
        self.0
            .deserialize_any(FixedVisitor(MapOnly(v)))
            .map_err(|_| FixedError)
    }
    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        v: V,
    ) -> Result<V::Value, FixedError> {
        self.0
            .deserialize_newtype_struct(name, FixedVisitor(v))
            .map_err(|_| FixedError)
    }
    serde::forward_to_deserialize_any! {bool i8 i16 i32 i64 i128 u8 u16 u32 u64 u128 f32 f64 char str string bytes byte_buf unit unit_struct seq tuple tuple_struct ignored_any}
    fn is_human_readable(&self) -> bool {
        self.0.is_human_readable()
    }
}
impl<'de, A: SeqAccess<'de>> SeqAccess<'de> for Fixed<A> {
    type Error = FixedError;
    fn next_element_seed<S: DeserializeSeed<'de>>(
        &mut self,
        s: S,
    ) -> Result<Option<S::Value>, FixedError> {
        self.0
            .next_element_seed(FixedSeed(s))
            .map_err(|_| FixedError)
    }
    // Do not propagate a size hint into a typed allocation.
}
impl<'de, A: MapAccess<'de>> MapAccess<'de> for Fixed<A> {
    type Error = FixedError;
    fn next_key_seed<S: DeserializeSeed<'de>>(
        &mut self,
        s: S,
    ) -> Result<Option<S::Value>, FixedError> {
        self.0.next_key_seed(FixedSeed(s)).map_err(|_| FixedError)
    }
    fn next_value_seed<S: DeserializeSeed<'de>>(&mut self, s: S) -> Result<S::Value, FixedError> {
        self.0.next_value_seed(FixedSeed(s)).map_err(|_| FixedError)
    }
}
impl<'de, A: EnumAccess<'de>> EnumAccess<'de> for Fixed<A> {
    type Error = FixedError;
    type Variant = Fixed<A::Variant>;
    fn variant_seed<S: DeserializeSeed<'de>>(
        self,
        s: S,
    ) -> Result<(S::Value, Self::Variant), FixedError> {
        let (v, a) = self.0.variant_seed(FixedSeed(s)).map_err(|_| FixedError)?;
        Ok((v, Fixed(a)))
    }
}
impl<'de, A: VariantAccess<'de>> VariantAccess<'de> for Fixed<A> {
    type Error = FixedError;
    fn unit_variant(self) -> Result<(), FixedError> {
        self.0.unit_variant().map_err(|_| FixedError)
    }
    fn newtype_variant_seed<S: DeserializeSeed<'de>>(self, s: S) -> Result<S::Value, FixedError> {
        self.0
            .newtype_variant_seed(FixedSeed(s))
            .map_err(|_| FixedError)
    }
    fn tuple_variant<V: Visitor<'de>>(self, n: usize, v: V) -> Result<V::Value, FixedError> {
        self.0
            .tuple_variant(n, FixedVisitor(v))
            .map_err(|_| FixedError)
    }
    fn struct_variant<V: Visitor<'de>>(
        self,
        f: &'static [&'static str],
        v: V,
    ) -> Result<V::Value, FixedError> {
        self.0
            .struct_variant(f, FixedVisitor(v))
            .map_err(|_| FixedError)
    }
}

// JSON object shapes must not accept serde's positional struct representation.
struct MapOnly<V>(V);
impl<'de, V: Visitor<'de>> Visitor<'de> for MapOnly<V> {
    type Value = V::Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("witness object")
    }
    fn visit_map<A: MapAccess<'de>>(self, a: A) -> Result<Self::Value, A::Error> {
        self.0.visit_map(a)
    }
}
// Includes the internally tagged step discriminant: derive's identifier visitor
// otherwise also accepts numeric variant indices, even for a JSON wire format.
struct StringOnly<V>(V);
impl<'de, V: Visitor<'de>> Visitor<'de> for StringOnly<V> {
    type Value = V::Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("witness string")
    }
    fn visit_str<E: de::Error>(self, s: &str) -> Result<Self::Value, E> {
        self.0.visit_str(s)
    }
    fn visit_borrowed_str<E: de::Error>(self, s: &'de str) -> Result<Self::Value, E> {
        self.0.visit_borrowed_str(s)
    }
    fn visit_string<E: de::Error>(self, s: String) -> Result<Self::Value, E> {
        self.0.visit_string(s)
    }
}
// Wire enum values are strings, without serde's externally-tagged object form.
struct StringEnumOnly<V>(V);
impl<'de, V: Visitor<'de>> Visitor<'de> for StringEnumOnly<V> {
    type Value = V::Value;
    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("witness enum string")
    }
    fn visit_str<E: de::Error>(self, s: &str) -> Result<Self::Value, E> {
        self.0.visit_enum(de::value::StrDeserializer::<E>::new(s))
    }
    fn visit_borrowed_str<E: de::Error>(self, s: &'de str) -> Result<Self::Value, E> {
        self.0
            .visit_enum(de::value::BorrowedStrDeserializer::<E>::new(s))
    }
    fn visit_string<E: de::Error>(self, s: String) -> Result<Self::Value, E> {
        self.0
            .visit_enum(de::value::StringDeserializer::<E>::new(s))
    }
}
