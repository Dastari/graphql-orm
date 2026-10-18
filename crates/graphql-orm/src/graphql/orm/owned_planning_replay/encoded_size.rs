//! Exact JSON layout arithmetic over borrowed typed values. This adapter emits
//! no JSON, escaped fragments, formatted integers, or intermediate strings.
use serde::ser::{self, Serialize, Serializer};
use std::fmt;
#[derive(Debug)]
pub(super) struct SizeError;
impl fmt::Display for SizeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("replay size limit")
    }
}
impl std::error::Error for SizeError {}
impl ser::Error for SizeError {
    fn custom<T: fmt::Display>(_: T) -> Self {
        Self
    }
}
type Result<T> = std::result::Result<T, SizeError>;
pub(super) struct Count {
    pub n: usize,
    pub maximum: usize,
}
impl Count {
    fn add(&mut self, n: usize) -> Result<()> {
        self.n = self
            .n
            .checked_add(n)
            .filter(|x| *x <= self.maximum)
            .ok_or(SizeError)?;
        Ok(())
    }
    fn string(&mut self, s: &str) -> Result<()> {
        self.add(2)?;
        self.add(s.len())?;
        for b in s.bytes() {
            match b {
                b'"' | b'\\' | b'\x08' | b'\x0c' | b'\n' | b'\r' | b'\t' => self.add(1)?,
                0..=31 => self.add(5)?,
                _ => {}
            }
        }
        Ok(())
    }
    fn unsigned(&mut self, mut n: u64) -> Result<()> {
        let mut digits = 1;
        while n >= 10 {
            digits += 1;
            n /= 10;
        }
        self.add(digits)
    }
    fn signed(&mut self, n: i64) -> Result<()> {
        if n < 0 {
            self.add(1)?;
        }
        self.unsigned(n.unsigned_abs())
    }
}
pub(super) struct Layout<'a> {
    pub count: &'a mut Count,
}
pub(super) struct Members<'a> {
    count: &'a mut Count,
    first: bool,
}
impl Members<'_> {
    fn separator(&mut self) -> Result<()> {
        if self.first {
            self.first = false;
            Ok(())
        } else {
            self.count.add(1)
        }
    }
}
impl ser::SerializeSeq for Members<'_> {
    type Ok = ();
    type Error = SizeError;
    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<()> {
        self.separator()?;
        value.serialize(Layout { count: self.count })
    }
    fn end(self) -> Result<()> {
        self.count.add(1)
    }
}
impl ser::SerializeStruct for Members<'_> {
    type Ok = ();
    type Error = SizeError;
    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        key: &'static str,
        value: &T,
    ) -> Result<()> {
        self.separator()?;
        self.count.string(key)?;
        self.count.add(1)?;
        value.serialize(Layout { count: self.count })
    }
    fn end(self) -> Result<()> {
        self.count.add(1)
    }
}
macro_rules! unsigned {($($name:ident:$ty:ty),*)=>{$(fn $name(self,v:$ty)->Result<()>{self.count.unsigned(v.into())})*};}
macro_rules! signed {($($name:ident:$ty:ty),*)=>{$(fn $name(self,v:$ty)->Result<()>{self.count.signed(v.into())})*};}
impl<'a> Serializer for Layout<'a> {
    type Ok = ();
    type Error = SizeError;
    type SerializeSeq = Members<'a>;
    type SerializeStruct = Members<'a>;
    type SerializeTuple = ser::Impossible<(), SizeError>;
    type SerializeTupleStruct = ser::Impossible<(), SizeError>;
    type SerializeTupleVariant = ser::Impossible<(), SizeError>;
    type SerializeMap = ser::Impossible<(), SizeError>;
    type SerializeStructVariant = ser::Impossible<(), SizeError>;
    fn serialize_bool(self, v: bool) -> Result<()> {
        self.count.add(if v { 4 } else { 5 })
    }
    unsigned!(serialize_u8:u8,serialize_u16:u16,serialize_u32:u32,serialize_u64:u64);
    signed!(serialize_i8:i8,serialize_i16:i16,serialize_i32:i32,serialize_i64:i64);
    fn serialize_str(self, s: &str) -> Result<()> {
        self.count.string(s)
    }
    fn serialize_none(self) -> Result<()> {
        self.count.add(4)
    }
    fn serialize_some<T: Serialize + ?Sized>(self, v: &T) -> Result<()> {
        v.serialize(self)
    }
    fn serialize_seq(self, _: Option<usize>) -> Result<Self::SerializeSeq> {
        self.count.add(1)?;
        Ok(Members {
            count: self.count,
            first: true,
        })
    }
    fn serialize_struct(self, _: &'static str, _: usize) -> Result<Self::SerializeStruct> {
        self.count.add(1)?;
        Ok(Members {
            count: self.count,
            first: true,
        })
    }
    // Closed supported wire shapes. Unknown formatting must not fall back to
    // Display, a generic tree, a custom formatter, or an unaccounted operation.
    fn serialize_f32(self, _: f32) -> Result<()> {
        Err(SizeError)
    }
    fn serialize_f64(self, _: f64) -> Result<()> {
        Err(SizeError)
    }
    fn serialize_char(self, _: char) -> Result<()> {
        Err(SizeError)
    }
    fn serialize_bytes(self, _: &[u8]) -> Result<()> {
        Err(SizeError)
    }
    fn serialize_unit(self) -> Result<()> {
        Err(SizeError)
    }
    fn serialize_unit_struct(self, _: &'static str) -> Result<()> {
        Err(SizeError)
    }
    fn serialize_unit_variant(self, _: &'static str, _: u32, _: &'static str) -> Result<()> {
        Err(SizeError)
    }
    fn serialize_newtype_struct<T: Serialize + ?Sized>(self, _: &'static str, _: &T) -> Result<()> {
        Err(SizeError)
    }
    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: &T,
    ) -> Result<()> {
        Err(SizeError)
    }
    fn serialize_tuple(self, _: usize) -> Result<Self::SerializeTuple> {
        Err(SizeError)
    }
    fn serialize_tuple_struct(
        self,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleStruct> {
        Err(SizeError)
    }
    fn serialize_tuple_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeTupleVariant> {
        Err(SizeError)
    }
    fn serialize_map(self, _: Option<usize>) -> Result<Self::SerializeMap> {
        Err(SizeError)
    }
    fn serialize_struct_variant(
        self,
        _: &'static str,
        _: u32,
        _: &'static str,
        _: usize,
    ) -> Result<Self::SerializeStructVariant> {
        Err(SizeError)
    }
    fn collect_str<T: fmt::Display + ?Sized>(self, _: &T) -> Result<()> {
        Err(SizeError)
    }
}
