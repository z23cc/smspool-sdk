use std::str::FromStr;

use rust_decimal::Decimal;
use serde::{Deserialize, Deserializer, de::DeserializeOwned};
use serde_json::Value;

use crate::types::{DecodedJson, StatusValue};

pub(crate) fn decimal_from_value<E>(value: Value) -> Result<Decimal, E>
where
    E: serde::de::Error,
{
    let text = match value {
        Value::String(value) => value,
        Value::Number(value) => value.to_string(),
        other => {
            return Err(E::custom(format!(
                "expected decimal string or number, got {other}"
            )));
        }
    };

    Decimal::from_str(&text)
        .or_else(|_| Decimal::from_scientific(&text))
        .map_err(|_| E::custom("invalid decimal value"))
}

pub(crate) fn deserialize_decimal<'de, D>(deserializer: D) -> Result<Decimal, D::Error>
where
    D: Deserializer<'de>,
{
    decimal_from_value(Value::deserialize(deserializer)?)
}

pub(crate) fn string_or_integer_from_value<E>(value: Value) -> Result<String, E>
where
    E: serde::de::Error,
{
    match value {
        Value::String(value) if !value.is_empty() => Ok(value),
        Value::String(_) => Err(E::custom("value must not be empty")),
        Value::Number(value) if value.is_i64() || value.is_u64() => Ok(value.to_string()),
        other => Err(E::custom(format!(
            "expected non-empty string or integer, got {other}"
        ))),
    }
}

pub(crate) fn deserialize_string_or_integer<'de, D>(deserializer: D) -> Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    string_or_integer_from_value(Value::deserialize(deserializer)?)
}

pub(crate) fn bool_from_value<E>(value: Value) -> Result<bool, E>
where
    E: serde::de::Error,
{
    match value {
        Value::Bool(value) => Ok(value),
        Value::Number(value) if value.as_i64() == Some(0) => Ok(false),
        Value::Number(value) if value.as_i64() == Some(1) => Ok(true),
        Value::String(value) => match value.as_str() {
            "0" | "false" => Ok(false),
            "1" | "true" => Ok(true),
            _ => Err(E::custom(
                "expected 0/1, boolean, or lowercase string boolean",
            )),
        },
        _ => Err(E::custom(
            "expected 0/1, boolean, or lowercase string boolean",
        )),
    }
}

pub(crate) fn deserialize_lenient_bool<'de, D>(deserializer: D) -> Result<bool, D::Error>
where
    D: Deserializer<'de>,
{
    bool_from_value(Value::deserialize(deserializer)?)
}

pub(crate) fn deserialize_status<'de, D>(deserializer: D) -> Result<StatusValue, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(StatusValue::from_value(Value::deserialize(deserializer)?))
}

pub(crate) fn deserialize_decoded_json<'de, D, T>(
    deserializer: D,
) -> Result<DecodedJson<T>, D::Error>
where
    D: Deserializer<'de>,
    T: DeserializeOwned,
{
    let raw = String::deserialize(deserializer)?;
    Ok(DecodedJson::from_raw(raw))
}

#[cfg(test)]
mod tests {
    use rust_decimal::Decimal;
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    struct DecimalFixture {
        #[serde(deserialize_with = "deserialize_decimal")]
        value: Decimal,
    }

    #[derive(Deserialize)]
    struct BoolFixture {
        #[serde(deserialize_with = "deserialize_lenient_bool")]
        value: bool,
    }

    #[derive(Debug, Deserialize, Eq, PartialEq)]
    struct Inner {
        code: u16,
    }

    #[derive(Deserialize)]
    struct DoubleEncodedFixture {
        #[serde(deserialize_with = "deserialize_decoded_json")]
        value: DecodedJson<Inner>,
    }

    #[test]
    fn decimal_accepts_string_and_number() {
        let string: DecimalFixture = serde_json::from_str(r#"{"value":"12.340"}"#).unwrap();
        let number: DecimalFixture = serde_json::from_str(r#"{"value":12.340}"#).unwrap();
        assert_eq!(string.value, Decimal::from_str("12.340").unwrap());
        assert_eq!(number.value, Decimal::from_str("12.340").unwrap());
    }

    #[test]
    fn lenient_boolean_is_narrow_and_case_sensitive() {
        for input in ["true", "1", r#""true""#, r#""1""#] {
            let fixture: BoolFixture =
                serde_json::from_str(&format!(r#"{{"value":{input}}}"#)).unwrap();
            assert!(fixture.value);
        }
        for input in ["false", "0", r#""false""#, r#""0""#] {
            let fixture: BoolFixture =
                serde_json::from_str(&format!(r#"{{"value":{input}}}"#)).unwrap();
            assert!(!fixture.value);
        }
        assert!(serde_json::from_str::<BoolFixture>(r#"{"value":"TRUE"}"#).is_err());
        assert!(serde_json::from_str::<BoolFixture>(r#"{"value":2}"#).is_err());
    }

    #[test]
    fn invalid_inner_json_does_not_fail_outer_decode() {
        let valid: DoubleEncodedFixture =
            serde_json::from_str(r#"{"value":"{\"code\":200}"}"#).unwrap();
        assert_eq!(valid.value.decoded(), Some(&Inner { code: 200 }));

        let invalid: DoubleEncodedFixture =
            serde_json::from_str(r#"{"value":"not-json"}"#).unwrap();
        assert_eq!(invalid.value.decoded(), None);
        assert_eq!(invalid.value.raw(), "not-json");
    }
}
