//! Generischer Finite-Check über Serde.
//!
//! [`all_finite`] läuft jeden [`serde::Serialize`]-Wert strukturell ab und
//! meldet, ob **alle** enthaltenen `f32`/`f64` endlich sind. Der Checker ist
//! ein Null-Allokations-Serializer, der ausschließlich Float-Werte prüft und
//! alles andere durchwinkt — damit ist er automatisch vollständig über
//! Primitive-Parameter, Sketch-Geometrie und Transforms (das Wire-Format der
//! Dokument-Typen), auch wenn künftig Varianten/Felder dazukommen.
//!
//! Motivation (v0.6.355): ein NaN aus dem Web-Sketch-Solver wandert sonst
//! ungeprüft in Octree/Edge-Walker-Numerik und reißt tief im Weave die ganze
//! WASM-Instanz ab (panic → abort; `catch_unwind` fängt auf
//! `wasm32-unknown-unknown` nichts). Der Weave-Einstieg
//! ([`crate::body::Body::ensure_brep`]) prüft Eingaben deshalb vorab.

use std::fmt;

use serde::ser::{
    Serialize, SerializeMap, SerializeSeq, SerializeStruct, SerializeStructVariant,
    SerializeTuple, SerializeTupleStruct, SerializeTupleVariant, Serializer,
};

/// `true` genau dann, wenn jeder `f32`/`f64` in `value` endlich ist
/// (kein NaN, kein ±∞).
pub fn all_finite<T: Serialize + ?Sized>(value: &T) -> bool {
    value.serialize(FiniteCheck).is_ok()
}

/// Marker-Fehler: irgendwo steckte ein nicht-endlicher Float.
#[derive(Debug)]
pub struct NonFinite;

impl fmt::Display for NonFinite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("non-finite float (NaN or infinity)")
    }
}

impl std::error::Error for NonFinite {}

impl serde::ser::Error for NonFinite {
    fn custom<T: fmt::Display>(_msg: T) -> Self {
        NonFinite
    }
}

/// Der eigentliche Prüf-Serializer: nur `serialize_f32`/`serialize_f64`
/// können fehlschlagen, alle Container rekursieren über `Self`.
struct FiniteCheck;

macro_rules! ok_leaf {
    ($($fn_name:ident: $ty:ty),* $(,)?) => {
        $(fn $fn_name(self, _v: $ty) -> Result<(), NonFinite> {
            Ok(())
        })*
    };
}

impl Serializer for FiniteCheck {
    type Ok = ();
    type Error = NonFinite;
    type SerializeSeq = FiniteCheck;
    type SerializeTuple = FiniteCheck;
    type SerializeTupleStruct = FiniteCheck;
    type SerializeTupleVariant = FiniteCheck;
    type SerializeMap = FiniteCheck;
    type SerializeStruct = FiniteCheck;
    type SerializeStructVariant = FiniteCheck;

    fn serialize_f32(self, v: f32) -> Result<(), NonFinite> {
        if v.is_finite() { Ok(()) } else { Err(NonFinite) }
    }

    fn serialize_f64(self, v: f64) -> Result<(), NonFinite> {
        if v.is_finite() { Ok(()) } else { Err(NonFinite) }
    }

    ok_leaf!(
        serialize_bool: bool,
        serialize_i8: i8,
        serialize_i16: i16,
        serialize_i32: i32,
        serialize_i64: i64,
        serialize_u8: u8,
        serialize_u16: u16,
        serialize_u32: u32,
        serialize_u64: u64,
        serialize_char: char,
        serialize_str: &str,
        serialize_bytes: &[u8],
    );

    fn serialize_none(self) -> Result<(), NonFinite> {
        Ok(())
    }

    fn serialize_some<T: Serialize + ?Sized>(self, value: &T) -> Result<(), NonFinite> {
        value.serialize(FiniteCheck)
    }

    fn serialize_unit(self) -> Result<(), NonFinite> {
        Ok(())
    }

    fn serialize_unit_struct(self, _name: &'static str) -> Result<(), NonFinite> {
        Ok(())
    }

    fn serialize_unit_variant(
        self,
        _name: &'static str,
        _idx: u32,
        _variant: &'static str,
    ) -> Result<(), NonFinite> {
        Ok(())
    }

    fn serialize_newtype_struct<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        value: &T,
    ) -> Result<(), NonFinite> {
        value.serialize(FiniteCheck)
    }

    fn serialize_newtype_variant<T: Serialize + ?Sized>(
        self,
        _name: &'static str,
        _idx: u32,
        _variant: &'static str,
        value: &T,
    ) -> Result<(), NonFinite> {
        value.serialize(FiniteCheck)
    }

    fn serialize_seq(self, _len: Option<usize>) -> Result<FiniteCheck, NonFinite> {
        Ok(FiniteCheck)
    }

    fn serialize_tuple(self, _len: usize) -> Result<FiniteCheck, NonFinite> {
        Ok(FiniteCheck)
    }

    fn serialize_tuple_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<FiniteCheck, NonFinite> {
        Ok(FiniteCheck)
    }

    fn serialize_tuple_variant(
        self,
        _name: &'static str,
        _idx: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<FiniteCheck, NonFinite> {
        Ok(FiniteCheck)
    }

    fn serialize_map(self, _len: Option<usize>) -> Result<FiniteCheck, NonFinite> {
        Ok(FiniteCheck)
    }

    fn serialize_struct(
        self,
        _name: &'static str,
        _len: usize,
    ) -> Result<FiniteCheck, NonFinite> {
        Ok(FiniteCheck)
    }

    fn serialize_struct_variant(
        self,
        _name: &'static str,
        _idx: u32,
        _variant: &'static str,
        _len: usize,
    ) -> Result<FiniteCheck, NonFinite> {
        Ok(FiniteCheck)
    }
}

impl SerializeSeq for FiniteCheck {
    type Ok = ();
    type Error = NonFinite;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), NonFinite> {
        value.serialize(FiniteCheck)
    }

    fn end(self) -> Result<(), NonFinite> {
        Ok(())
    }
}

impl SerializeTuple for FiniteCheck {
    type Ok = ();
    type Error = NonFinite;

    fn serialize_element<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), NonFinite> {
        value.serialize(FiniteCheck)
    }

    fn end(self) -> Result<(), NonFinite> {
        Ok(())
    }
}

impl SerializeTupleStruct for FiniteCheck {
    type Ok = ();
    type Error = NonFinite;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), NonFinite> {
        value.serialize(FiniteCheck)
    }

    fn end(self) -> Result<(), NonFinite> {
        Ok(())
    }
}

impl SerializeTupleVariant for FiniteCheck {
    type Ok = ();
    type Error = NonFinite;

    fn serialize_field<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), NonFinite> {
        value.serialize(FiniteCheck)
    }

    fn end(self) -> Result<(), NonFinite> {
        Ok(())
    }
}

impl SerializeMap for FiniteCheck {
    type Ok = ();
    type Error = NonFinite;

    fn serialize_key<T: Serialize + ?Sized>(&mut self, key: &T) -> Result<(), NonFinite> {
        key.serialize(FiniteCheck)
    }

    fn serialize_value<T: Serialize + ?Sized>(&mut self, value: &T) -> Result<(), NonFinite> {
        value.serialize(FiniteCheck)
    }

    fn end(self) -> Result<(), NonFinite> {
        Ok(())
    }
}

impl SerializeStruct for FiniteCheck {
    type Ok = ();
    type Error = NonFinite;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _key: &'static str,
        value: &T,
    ) -> Result<(), NonFinite> {
        value.serialize(FiniteCheck)
    }

    fn end(self) -> Result<(), NonFinite> {
        Ok(())
    }
}

impl SerializeStructVariant for FiniteCheck {
    type Ok = ();
    type Error = NonFinite;

    fn serialize_field<T: Serialize + ?Sized>(
        &mut self,
        _key: &'static str,
        value: &T,
    ) -> Result<(), NonFinite> {
        value.serialize(FiniteCheck)
    }

    fn end(self) -> Result<(), NonFinite> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::all_finite;
    use fielddraft_protocol::{Primitive, Sketch2d, Sketch2dPrimitive, Transform};

    #[test]
    fn finite_primitives_pass() {
        assert!(all_finite(&Primitive::Box {
            center: [1.0, -2.0, 3.0],
            half_extents: [4.0, 5.0, 6.0],
        }));
        assert!(all_finite(&Transform::IDENTITY));
        assert!(all_finite(&Sketch2d::Prim(Sketch2dPrimitive::Polygon {
            vertices: vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0]],
        })));
    }

    #[test]
    fn nan_and_infinity_are_detected_at_any_depth() {
        assert!(!all_finite(&f32::NAN));
        assert!(!all_finite(&f64::INFINITY));
        assert!(!all_finite(&Primitive::Box {
            center: [0.0, f32::NAN, 0.0],
            half_extents: [1.0, 1.0, 1.0],
        }));
        // Ein NaN-Vertex tief in einer Sketch-Polygon-Liste — der Fall, den
        // der Web-Sketch-Solver produzieren kann.
        assert!(!all_finite(&Sketch2d::Prim(Sketch2dPrimitive::Polygon {
            vertices: vec![[0.0, 0.0], [f32::NAN, 5.0], [10.0, 10.0]],
        })));
    }
}
