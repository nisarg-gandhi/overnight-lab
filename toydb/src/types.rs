//! SQL values and their two binary encodings.
//!
//! * **Row encoding** — how a whole row is stored as a B+tree *value*.
//!   Compact and self-describing (each value carries a type tag).
//! * **Key encoding** — how a primary-key value is stored as a B+tree *key*.
//!   It is *order-preserving*: comparing two encoded keys byte-by-byte
//!   (`memcmp`) gives the same answer as comparing the values. That is what
//!   lets a B+tree that only knows about bytes answer `WHERE id < 10` with a
//!   range scan.
//!
//! ```text
//! Row:   [ncols u16] [tag u8][payload] [tag u8][payload] ...
//!          tag 0 NULL     (no payload)
//!          tag 1 INTEGER  i64, little-endian
//!          tag 2 REAL     f64 bits, little-endian
//!          tag 3 TEXT     u32 length + UTF-8 bytes
//!          tag 4 BOOLEAN  u8 (0/1)
//!
//! Key:   [tag u8] [order-preserving payload]
//!          INTEGER  i64 with sign bit flipped, big-endian
//!                   (-1 => 7F FF .. FF, 0 => 80 00 .. 00, 1 => 80 00 .. 01)
//!          REAL     positive: bits with sign flipped; negative: all bits
//!                   inverted; big-endian
//!          TEXT     bytes, 0x00 escaped as 00 FF, terminated by 00 00
//!          BOOLEAN  00 / 01
//! ```

use std::cmp::Ordering;
use std::fmt;

use crate::error::{corrupt, Error, Result};

/// Column types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DataType {
    Integer,
    Real,
    Text,
    Boolean,
}

impl fmt::Display for DataType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            DataType::Integer => "INTEGER",
            DataType::Real => "REAL",
            DataType::Text => "TEXT",
            DataType::Boolean => "BOOLEAN",
        })
    }
}

/// A single SQL value.
#[derive(Clone, Debug)]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Boolean(bool),
}

/// A row is just a vector of values, in column order.
pub type Row = Vec<Value>;

impl Value {
    pub fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }

    pub fn data_type(&self) -> Option<DataType> {
        match self {
            Value::Null => None,
            Value::Integer(_) => Some(DataType::Integer),
            Value::Real(_) => Some(DataType::Real),
            Value::Text(_) => Some(DataType::Text),
            Value::Boolean(_) => Some(DataType::Boolean),
        }
    }

    pub fn type_name(&self) -> &'static str {
        match self {
            Value::Null => "NULL",
            Value::Integer(_) => "INTEGER",
            Value::Real(_) => "REAL",
            Value::Text(_) => "TEXT",
            Value::Boolean(_) => "BOOLEAN",
        }
    }

    /// Convert a value for storage in a column of type `ty`. The only
    /// implicit conversion is INTEGER -> REAL; anything else is an error.
    pub fn coerce_to(self, ty: DataType) -> Result<Value> {
        match (self, ty) {
            (Value::Null, _) => Ok(Value::Null),
            (Value::Integer(i), DataType::Real) => Ok(Value::Real(i as f64)),
            (v, ty) if v.data_type() == Some(ty) => Ok(v),
            (v, ty) => Err(Error::Query(format!(
                "type mismatch: cannot store {} value {v} in {ty} column",
                v.type_name()
            ))),
        }
    }

    /// SQL comparison: `None` if either side is NULL (the result is
    /// unknown). Numbers compare across INTEGER/REAL; comparing different
    /// non-numeric types is an error.
    pub fn sql_cmp(&self, other: &Value) -> Result<Option<Ordering>> {
        use Value::*;
        Ok(match (self, other) {
            (Null, _) | (_, Null) => None,
            (Integer(a), Integer(b)) => Some(a.cmp(b)),
            (Integer(a), Real(b)) => (*a as f64).partial_cmp(b),
            (Real(a), Integer(b)) => a.partial_cmp(&(*b as f64)),
            (Real(a), Real(b)) => a.partial_cmp(b),
            (Text(a), Text(b)) => Some(a.cmp(b)),
            (Boolean(a), Boolean(b)) => Some(a.cmp(b)),
            (a, b) => {
                return Err(Error::Query(format!(
                    "cannot compare {} with {}",
                    a.type_name(),
                    b.type_name()
                )))
            }
        })
    }

    /// A *total* order used for ORDER BY, GROUP BY and DISTINCT, where we
    /// need every pair of values to be comparable: NULL sorts first, then
    /// booleans, numbers, and text.
    pub fn total_cmp(&self, other: &Value) -> Ordering {
        use Value::*;
        fn rank(v: &Value) -> u8 {
            match v {
                Null => 0,
                Boolean(_) => 1,
                Integer(_) | Real(_) => 2,
                Text(_) => 3,
            }
        }
        match (self, other) {
            (Integer(a), Integer(b)) => a.cmp(b),
            (Integer(a), Real(b)) => (*a as f64).total_cmp(b),
            (Real(a), Integer(b)) => a.total_cmp(&(*b as f64)),
            (Real(a), Real(b)) => a.total_cmp(b),
            (Text(a), Text(b)) => a.cmp(b),
            (Boolean(a), Boolean(b)) => a.cmp(b),
            (a, b) => rank(a).cmp(&rank(b)),
        }
    }
}

// Equality and ordering for Value follow `total_cmp`, so values can be used
// as BTreeMap keys (GROUP BY) and sorted (ORDER BY).
impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        self.total_cmp(other) == Ordering::Equal
    }
}
impl Eq for Value {}
impl PartialOrd for Value {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for Value {
    fn cmp(&self, other: &Self) -> Ordering {
        self.total_cmp(other)
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Value::Null => f.write_str("NULL"),
            Value::Integer(i) => write!(f, "{i}"),
            Value::Real(r) => {
                // Always show a decimal point so REALs are recognisable.
                if r.is_finite() && r.fract() == 0.0 && r.abs() < 1e15 {
                    write!(f, "{r:.1}")
                } else {
                    write!(f, "{r}")
                }
            }
            Value::Text(s) => f.write_str(s),
            Value::Boolean(b) => f.write_str(if *b { "TRUE" } else { "FALSE" }),
        }
    }
}

impl Value {
    /// Render as a SQL literal (strings quoted), used by EXPLAIN and when
    /// regenerating SQL text.
    pub fn to_sql(&self) -> String {
        match self {
            Value::Text(s) => format!("'{}'", s.replace('\'', "''")),
            v => v.to_string(),
        }
    }
}

// --- row encoding ---------------------------------------------------------------

const TAG_NULL: u8 = 0;
const TAG_INT: u8 = 1;
const TAG_REAL: u8 = 2;
const TAG_TEXT: u8 = 3;
const TAG_BOOL: u8 = 4;

/// Serialize a row (the B+tree value for a table entry).
pub fn encode_row(row: &[Value]) -> Vec<u8> {
    let mut out = Vec::with_capacity(2 + row.len() * 9);
    out.extend_from_slice(&(row.len() as u16).to_le_bytes());
    for v in row {
        match v {
            Value::Null => out.push(TAG_NULL),
            Value::Integer(i) => {
                out.push(TAG_INT);
                out.extend_from_slice(&i.to_le_bytes());
            }
            Value::Real(r) => {
                out.push(TAG_REAL);
                out.extend_from_slice(&r.to_bits().to_le_bytes());
            }
            Value::Text(s) => {
                out.push(TAG_TEXT);
                out.extend_from_slice(&(s.len() as u32).to_le_bytes());
                out.extend_from_slice(s.as_bytes());
            }
            Value::Boolean(b) => {
                out.push(TAG_BOOL);
                out.push(*b as u8);
            }
        }
    }
    out
}

/// Deserialize a row produced by [`encode_row`].
pub fn decode_row(buf: &[u8]) -> Result<Row> {
    let take = |pos: &mut usize, n: usize| -> Result<&[u8]> {
        let s = buf
            .get(*pos..*pos + n)
            .ok_or_else(|| corrupt!("truncated row"))?;
        *pos += n;
        Ok(s)
    };
    let mut pos = 0;
    let n = u16::from_le_bytes(take(&mut pos, 2)?.try_into().unwrap()) as usize;
    let mut row = Vec::with_capacity(n);
    for _ in 0..n {
        let tag = take(&mut pos, 1)?[0];
        row.push(match tag {
            TAG_NULL => Value::Null,
            TAG_INT => Value::Integer(i64::from_le_bytes(take(&mut pos, 8)?.try_into().unwrap())),
            TAG_REAL => Value::Real(f64::from_bits(u64::from_le_bytes(
                take(&mut pos, 8)?.try_into().unwrap(),
            ))),
            TAG_TEXT => {
                let len = u32::from_le_bytes(take(&mut pos, 4)?.try_into().unwrap()) as usize;
                let bytes = take(&mut pos, len)?;
                Value::Text(
                    String::from_utf8(bytes.to_vec()).map_err(|_| corrupt!("invalid UTF-8 in row"))?,
                )
            }
            TAG_BOOL => Value::Boolean(take(&mut pos, 1)?[0] != 0),
            t => return Err(corrupt!("unknown value tag {t}")),
        });
    }
    Ok(row)
}

// --- key encoding -------------------------------------------------------------------

/// Append the order-preserving encoding of `v` to `out`.
pub fn encode_key_into(v: &Value, out: &mut Vec<u8>) {
    match v {
        Value::Null => out.push(TAG_NULL),
        Value::Boolean(b) => {
            out.push(TAG_BOOL);
            out.push(*b as u8);
        }
        Value::Integer(i) => {
            out.push(TAG_INT);
            out.extend_from_slice(&((*i as u64) ^ (1 << 63)).to_be_bytes());
        }
        Value::Real(r) => {
            out.push(TAG_REAL);
            // -0.0 and 0.0 are equal in SQL, so they must encode the same.
            let bits = if *r == 0.0 { 0 } else { r.to_bits() };
            let ordered = if bits >> 63 == 1 { !bits } else { bits ^ (1 << 63) };
            out.extend_from_slice(&ordered.to_be_bytes());
        }
        Value::Text(s) => {
            out.push(TAG_TEXT);
            for &b in s.as_bytes() {
                out.push(b);
                if b == 0 {
                    out.push(0xFF);
                }
            }
            out.extend_from_slice(&[0, 0]);
        }
    }
}

/// Order-preserving encoding of a single value.
pub fn encode_key(v: &Value) -> Vec<u8> {
    let mut out = Vec::with_capacity(10);
    encode_key_into(v, &mut out);
    out
}

/// Inverse of [`encode_key`] for integer keys (used to find the next rowid).
pub fn decode_int_key(k: &[u8]) -> Result<i64> {
    if k.len() != 9 || k[0] != TAG_INT {
        return Err(corrupt!("not an integer key"));
    }
    Ok((u64::from_be_bytes(k[1..9].try_into().unwrap()) ^ (1 << 63)) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn arb_value() -> impl Strategy<Value = Value> {
        prop_oneof![
            Just(Value::Null),
            any::<i64>().prop_map(Value::Integer),
            any::<f64>().prop_filter("no NaN", |f| !f.is_nan()).prop_map(Value::Real),
            ".{0,20}".prop_map(Value::Text),
            any::<bool>().prop_map(Value::Boolean),
        ]
    }

    proptest! {
        #[test]
        fn row_round_trip(row in prop::collection::vec(arb_value(), 0..10)) {
            let decoded = decode_row(&encode_row(&row)).unwrap();
            prop_assert_eq!(decoded.len(), row.len());
            for (a, b) in decoded.iter().zip(&row) {
                // Compare via Debug so -0.0 vs 0.0 and types are exact.
                prop_assert_eq!(format!("{a:?}"), format!("{b:?}"));
            }
        }

        #[test]
        fn int_keys_preserve_order(a in any::<i64>(), b in any::<i64>()) {
            let (ka, kb) = (encode_key(&Value::Integer(a)), encode_key(&Value::Integer(b)));
            prop_assert_eq!(ka.cmp(&kb), a.cmp(&b));
            prop_assert_eq!(decode_int_key(&ka).unwrap(), a);
        }

        #[test]
        fn real_keys_preserve_order(a in any::<f64>(), b in any::<f64>()) {
            prop_assume!(!a.is_nan() && !b.is_nan());
            let (ka, kb) = (encode_key(&Value::Real(a)), encode_key(&Value::Real(b)));
            prop_assert_eq!(ka.cmp(&kb), a.partial_cmp(&b).unwrap());
        }

        #[test]
        fn text_keys_preserve_order(a in "[a\\x00b]{0,8}", b in "[a\\x00b]{0,8}") {
            let (ka, kb) = (encode_key(&Value::Text(a.clone())), encode_key(&Value::Text(b.clone())));
            prop_assert_eq!(ka.cmp(&kb), a.cmp(&b));
        }
    }

    #[test]
    fn coercion() {
        assert!(matches!(Value::Integer(3).coerce_to(DataType::Real), Ok(Value::Real(r)) if r == 3.0));
        assert!(Value::Text("x".into()).coerce_to(DataType::Integer).is_err());
        assert!(Value::Null.coerce_to(DataType::Boolean).unwrap().is_null());
    }

    #[test]
    fn sql_comparison_semantics() {
        assert_eq!(Value::Integer(1).sql_cmp(&Value::Real(1.5)).unwrap(), Some(Ordering::Less));
        assert_eq!(Value::Null.sql_cmp(&Value::Integer(1)).unwrap(), None);
        assert!(Value::Text("a".into()).sql_cmp(&Value::Integer(1)).is_err());
        assert_eq!(Value::Real(2.0).to_string(), "2.0");
    }
}
