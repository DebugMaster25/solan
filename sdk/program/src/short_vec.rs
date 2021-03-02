#![allow(clippy::integer_arithmetic)]
use serde::{
    de::{self, Deserializer, SeqAccess, Visitor},
    ser::{self, SerializeTuple, Serializer},
    {Deserialize, Serialize},
};
use std::{fmt, marker::PhantomData, mem::size_of};

/// Same as u16, but serialized with 1 to 3 bytes. If the value is above
/// 0x7f, the top bit is set and the remaining value is stored in the next
/// bytes. Each byte follows the same pattern until the 3rd byte. The 3rd
/// byte, if needed, uses all 8 bits to store the last byte of the original
/// value.
#[derive(AbiExample)]
pub struct ShortU16(pub u16);

impl Serialize for ShortU16 {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        // Pass a non-zero value to serialize_tuple() so that serde_json will
        // generate an open bracket.
        let mut seq = serializer.serialize_tuple(1)?;

        let mut rem_val = self.0;
        loop {
            let mut elem = (rem_val & 0x7f) as u8;
            rem_val >>= 7;
            if rem_val == 0 {
                seq.serialize_element(&elem)?;
                break;
            } else {
                elem |= 0x80;
                seq.serialize_element(&elem)?;
            }
        }
        seq.end()
    }
}

enum VisitResult {
    Done(usize, usize),
    More(usize, usize),
    Err,
}

fn visit_byte(elem: u8, val: usize, size: usize) -> VisitResult {
    let val = val | (elem as usize & 0x7f) << (size * 7);
    let size = size + 1;
    let more = elem as usize & 0x80 == 0x80;

    if size > size_of::<u16>() + 1 {
        VisitResult::Err
    } else if more {
        VisitResult::More(val, size)
    } else {
        VisitResult::Done(val, size)
    }
}

struct ShortU16Visitor;

impl<'de> Visitor<'de> for ShortU16Visitor {
    type Value = ShortU16;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a ShortU16")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<ShortU16, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut val: usize = 0;
        let mut size: usize = 0;
        loop {
            let elem: u8 = seq
                .next_element()?
                .ok_or_else(|| de::Error::invalid_length(size, &self))?;

            match visit_byte(elem, val, size) {
                VisitResult::Done(l, _) => {
                    val = l;
                    break;
                }
                VisitResult::More(l, s) => {
                    val = l;
                    size = s;
                }
                VisitResult::Err => return Err(de::Error::invalid_length(size + 1, &self)),
            }
        }

        Ok(ShortU16(val as u16))
    }
}

impl<'de> Deserialize<'de> for ShortU16 {
    fn deserialize<D>(deserializer: D) -> Result<ShortU16, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_tuple(3, ShortU16Visitor)
    }
}

/// If you don't want to use the ShortVec newtype, you can do ShortVec
/// serialization on an ordinary vector with the following field annotation:
///
/// #[serde(with = "short_vec")]
///
pub fn serialize<S: Serializer, T: Serialize>(
    elements: &[T],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    // Pass a non-zero value to serialize_tuple() so that serde_json will
    // generate an open bracket.
    let mut seq = serializer.serialize_tuple(1)?;

    let len = elements.len();
    if len > std::u16::MAX as usize {
        return Err(ser::Error::custom("length larger than u16"));
    }
    let short_len = ShortU16(len as u16);
    seq.serialize_element(&short_len)?;

    for element in elements {
        seq.serialize_element(element)?;
    }
    seq.end()
}

struct ShortVecVisitor<T> {
    _t: PhantomData<T>,
}

impl<'de, T> Visitor<'de> for ShortVecVisitor<T>
where
    T: Deserialize<'de>,
{
    type Value = Vec<T>;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("a Vec with a multi-byte length")
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Vec<T>, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let short_len: ShortU16 = seq
            .next_element()?
            .ok_or_else(|| de::Error::invalid_length(0, &self))?;
        let len = short_len.0 as usize;

        let mut result = Vec::with_capacity(len);
        for i in 0..len {
            let elem = seq
                .next_element()?
                .ok_or_else(|| de::Error::invalid_length(i, &self))?;
            result.push(elem);
        }
        Ok(result)
    }
}

/// If you don't want to use the ShortVec newtype, you can do ShortVec
/// deserialization on an ordinary vector with the following field annotation:
///
/// #[serde(with = "short_vec")]
///
pub fn deserialize<'de, D, T>(deserializer: D) -> Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    let visitor = ShortVecVisitor { _t: PhantomData };
    deserializer.deserialize_tuple(std::usize::MAX, visitor)
}

pub struct ShortVec<T>(pub Vec<T>);

impl<T: Serialize> Serialize for ShortVec<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serialize(&self.0, serializer)
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for ShortVec<T> {
    fn deserialize<D>(deserializer: D) -> Result<ShortVec<T>, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserialize(deserializer).map(ShortVec)
    }
}

/// Return the decoded value and how many bytes it consumed.
#[allow(clippy::result_unit_err)]
pub fn decode_len(bytes: &[u8]) -> Result<(usize, usize), ()> {
    let mut len = 0;
    let mut size = 0;
    for byte in bytes.iter() {
        match visit_byte(*byte, len, size) {
            VisitResult::More(l, s) => {
                len = l;
                size = s;
            }
            VisitResult::Done(len, size) => return Ok((len, size)),
            VisitResult::Err => return Err(()),
        }
    }
    Err(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use assert_matches::assert_matches;
    use bincode::{deserialize, serialize};

    /// Return the serialized length.
    fn encode_len(len: u16) -> Vec<u8> {
        bincode::serialize(&ShortU16(len)).unwrap()
    }

    fn assert_len_encoding(len: u16, bytes: &[u8]) {
        assert_eq!(encode_len(len), bytes, "unexpected usize encoding");
        assert_eq!(
            decode_len(bytes).unwrap(),
            (len as usize, bytes.len()),
            "unexpected usize decoding"
        );
    }

    #[test]
    fn test_short_vec_encode_len() {
        assert_len_encoding(0x0, &[0x0]);
        assert_len_encoding(0x7f, &[0x7f]);
        assert_len_encoding(0x80, &[0x80, 0x01]);
        assert_len_encoding(0xff, &[0xff, 0x01]);
        assert_len_encoding(0x100, &[0x80, 0x02]);
        assert_len_encoding(0x7fff, &[0xff, 0xff, 0x01]);
        assert_len_encoding(0xffff, &[0xff, 0xff, 0x03]);
    }

    fn assert_good_deserialized_value(value: u16, bytes: &[u8]) {
        assert_eq!(value, deserialize::<ShortU16>(bytes).unwrap().0);
    }

    fn assert_bad_deserialized_value(bytes: &[u8]) {
        assert!(deserialize::<ShortU16>(bytes).is_err());
    }

    #[test]
    fn test_deserialize() {
        assert_good_deserialized_value(0x0000, &[0x00]);
        assert_good_deserialized_value(0x007f, &[0x7f]);
        assert_good_deserialized_value(0x0080, &[0x80, 0x01]);
        assert_good_deserialized_value(0x00ff, &[0xff, 0x01]);
        assert_good_deserialized_value(0x0100, &[0x80, 0x02]);
        assert_good_deserialized_value(0x07ff, &[0xff, 0x0f]);
        assert_good_deserialized_value(0x3fff, &[0xff, 0x7f]);
        assert_good_deserialized_value(0x4000, &[0x80, 0x80, 0x01]);
        assert_good_deserialized_value(0xffff, &[0xff, 0xff, 0x03]);

        // aliases
        // 0x0000
        assert_bad_deserialized_value(&[0x80, 0x00]);
        assert_bad_deserialized_value(&[0x80, 0x80, 0x00]);
        // 0x007f
        assert_bad_deserialized_value(&[0xff, 0x00]);
        assert_bad_deserialized_value(&[0xff, 0x80, 0x00]);
        // 0x0080
        assert_bad_deserialized_value(&[0x80, 0x81, 0x00]);
        // 0x00ff
        assert_bad_deserialized_value(&[0xff, 0x81, 0x00]);
        // 0x0100
        assert_bad_deserialized_value(&[0x80, 0x82, 0x00]);
        // 0x07ff
        assert_bad_deserialized_value(&[0xff, 0x8f, 0x00]);
        // 0x3fff
        assert_bad_deserialized_value(&[0xff, 0xff, 0x00]);

        // too short
        assert_bad_deserialized_value(&[]);
        assert_bad_deserialized_value(&[0x80]);

        // too long
        assert_bad_deserialized_value(&[0x80, 0x80, 0x80, 0x00]);

        // too large
        // 0x0001_0000
        assert_bad_deserialized_value(&[0x80, 0x80, 0x04]);
        // 0x0001_8000
        assert_bad_deserialized_value(&[0x80, 0x80, 0x06]);
    }

    #[test]
    fn test_short_vec_u8() {
        let vec = ShortVec(vec![4u8; 32]);
        let bytes = serialize(&vec).unwrap();
        assert_eq!(bytes.len(), vec.0.len() + 1);

        let vec1: ShortVec<u8> = deserialize(&bytes).unwrap();
        assert_eq!(vec.0, vec1.0);
    }

    #[test]
    fn test_short_vec_u8_too_long() {
        let vec = ShortVec(vec![4u8; std::u16::MAX as usize]);
        assert_matches!(serialize(&vec), Ok(_));

        let vec = ShortVec(vec![4u8; std::u16::MAX as usize + 1]);
        assert_matches!(serialize(&vec), Err(_));
    }

    #[test]
    fn test_short_vec_json() {
        let vec = ShortVec(vec![0, 1, 2]);
        let s = serde_json::to_string(&vec).unwrap();
        assert_eq!(s, "[[3],0,1,2]");
    }

    #[test]
    fn test_short_vec_aliased_length() {
        let bytes = [
            0x81, 0x80, 0x00, // 3-byte alias of 1
            0x00,
        ];
        assert!(deserialize::<ShortVec<u8>>(&bytes).is_err());
    }
}
