use std::{fmt, str::FromStr};

use rust_decimal::Decimal;
use secrecy::{ExposeSecret, SecretString};
use serde::{de::DeserializeOwned, Deserialize, Deserializer, Serialize, Serializer};
use serde_json::Value;

/// Safe validation error that never retains or prints the rejected value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub struct InvalidValue {
    field: &'static str,
    reason: &'static str,
}

impl InvalidValue {
    pub const fn new(field: &'static str, reason: &'static str) -> Self {
        Self { field, reason }
    }

    pub const fn field(&self) -> &'static str {
        self.field
    }

    pub const fn reason(&self) -> &'static str {
        self.reason
    }
}

impl fmt::Display for InvalidValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "invalid {}: {}", self.field, self.reason)
    }
}

impl std::error::Error for InvalidValue {}

macro_rules! identifier {
    ($name:ident, $field:literal) => {
        #[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, InvalidValue> {
                let value = value.into();
                if value.is_empty() {
                    return Err(InvalidValue::new($field, "must not be empty"));
                }
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl FromStr for $name {
            type Err = InvalidValue;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Self::new(value)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(&self.0)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = crate::de::deserialize_string_or_integer(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

identifier!(OrderId, "order_id");
identifier!(CountryId, "country_id");
identifier!(ServiceId, "service_id");
identifier!(PoolId, "pool_id");
identifier!(PreorderId, "preorder_id");
identifier!(RentalId, "rental_id");
identifier!(RentalCode, "rental_code");
identifier!(PlanId, "plan_id");
identifier!(TransactionId, "transaction_id");
identifier!(BusinessUserId, "business_user_id");

macro_rules! decimal_type {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
        pub struct $name(Decimal);

        impl $name {
            pub const fn new(value: Decimal) -> Self {
                Self(value)
            }

            pub const fn value(self) -> Decimal {
                self.0
            }
        }

        impl From<Decimal> for $name {
            fn from(value: Decimal) -> Self {
                Self(value)
            }
        }

        impl FromStr for $name {
            type Err = rust_decimal::Error;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Decimal::from_str(value).map(Self)
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: Serializer,
            {
                serializer.serialize_str(&self.0.to_string())
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                crate::de::deserialize_decimal(deserializer).map(Self)
            }
        }
    };
}

decimal_type!(Money);
decimal_type!(DecimalValue);

/// Signed, ephemeral money difference used for refund corroboration.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct SignedMoneyDelta(Decimal);

impl SignedMoneyDelta {
    pub const fn new(value: Decimal) -> Self {
        Self(value)
    }

    pub const fn value(self) -> Decimal {
        self.0
    }
}

impl fmt::Display for SignedMoneyDelta {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

macro_rules! unit_type {
    ($name:ident, $inner:ty) => {
        #[derive(
            Clone,
            Copy,
            Debug,
            Default,
            Eq,
            Hash,
            Ord,
            PartialEq,
            PartialOrd,
            Serialize,
            Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(pub $inner);

        impl $name {
            pub const fn new(value: $inner) -> Self {
                Self(value)
            }

            pub const fn value(self) -> $inner {
                self.0
            }
        }
    };
}

unit_type!(Cents, u64);
unit_type!(Seconds, u64);
unit_type!(Hours, u64);
unit_type!(Days, u32);
unit_type!(UnixTimestamp, i64);

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct VendorDateTime(String);

impl VendorDateTime {
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidValue> {
        let value = value.into();
        if value.is_empty() {
            return Err(InvalidValue::new("vendor_date_time", "must not be empty"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

macro_rules! sensitive_string {
    ($name:ident, $field:literal) => {
        #[derive(Clone)]
        pub struct $name(SecretString);

        impl $name {
            pub fn new(value: impl Into<String>) -> Result<Self, InvalidValue> {
                let value = value.into();
                if value.is_empty() {
                    return Err(InvalidValue::new($field, "must not be empty"));
                }
                Ok(Self(SecretString::new(value)))
            }

            /// Explicitly exposes customer or credential data.
            pub fn expose(&self) -> &str {
                self.0.expose_secret()
            }
        }

        impl PartialEq for $name {
            fn eq(&self, other: &Self) -> bool {
                self.expose() == other.expose()
            }
        }

        impl Eq for $name {}

        impl fmt::Debug for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str(concat!(stringify!($name), "([REDACTED])"))
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("[REDACTED]")
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::new(value).map_err(serde::de::Error::custom)
            }
        }
    };
}

sensitive_string!(SmsText, "sms_text");
sensitive_string!(ActivationToken, "activation_token");
sensitive_string!(EsimCredential, "esim_credential");
sensitive_string!(Password, "password");

#[derive(Clone)]
pub struct PhoneNumber(SecretString);

impl PhoneNumber {
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidValue> {
        let value = value.into();
        if value.is_empty() {
            return Err(InvalidValue::new("phone_number", "must not be empty"));
        }
        Ok(Self(SecretString::new(value)))
    }

    /// Explicitly exposes the normalized vendor value.
    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

impl PartialEq for PhoneNumber {
    fn eq(&self, other: &Self) -> bool {
        self.expose() == other.expose()
    }
}

impl Eq for PhoneNumber {}

impl fmt::Debug for PhoneNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("PhoneNumber([REDACTED])")
    }
}

impl fmt::Display for PhoneNumber {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("[REDACTED]")
    }
}

impl<'de> Deserialize<'de> for PhoneNumber {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = crate::de::deserialize_string_or_integer(deserializer)?;
        Self::new(value).map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, PartialEq)]
#[non_exhaustive]
pub enum StatusValue {
    Integer(i64),
    String(String),
    Boolean(bool),
    Null,
    Other(Value),
}

impl fmt::Debug for StatusValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Integer(value) => formatter.debug_tuple("Integer").field(value).finish(),
            Self::String(value) => formatter.debug_tuple("String").field(value).finish(),
            Self::Boolean(value) => formatter.debug_tuple("Boolean").field(value).finish(),
            Self::Null => formatter.write_str("Null"),
            Self::Other(_) => formatter.write_str("Other([REDACTED])"),
        }
    }
}

impl StatusValue {
    pub(crate) fn from_value(value: Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Bool(value) => Self::Boolean(value),
            Value::String(value) => Self::String(value),
            Value::Number(value) if value.is_i64() => Self::Integer(value.as_i64().unwrap()),
            other => Self::Other(other),
        }
    }
}

impl<'de> Deserialize<'de> for StatusValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        crate::de::deserialize_status(deserializer)
    }
}

impl Serialize for StatusValue {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Self::Integer(value) => serializer.serialize_i64(*value),
            Self::String(value) => serializer.serialize_str(value),
            Self::Boolean(value) => serializer.serialize_bool(*value),
            Self::Null => serializer.serialize_none(),
            Self::Other(value) => value.serialize(serializer),
        }
    }
}

/// Arbitrary provider JSON retained for explicit inspection but omitted from typed `Debug` output.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RedactedValue(Value);

impl RedactedValue {
    pub fn new(value: Value) -> Self {
        Self(value)
    }

    pub fn expose(&self) -> &Value {
        &self.0
    }
}

impl fmt::Debug for RedactedValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("RedactedValue([REDACTED])")
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct DecodedJson<T> {
    raw: String,
    decoded: Option<T>,
}

impl<T> DecodedJson<T>
where
    T: DeserializeOwned,
{
    pub fn from_raw(raw: String) -> Self {
        let decoded = serde_json::from_str(&raw).ok();
        Self { raw, decoded }
    }
}

impl<T> DecodedJson<T> {
    /// Explicitly exposes the original inner JSON string.
    pub fn raw(&self) -> &str {
        &self.raw
    }

    pub fn decoded(&self) -> Option<&T> {
        self.decoded.as_ref()
    }

    pub fn into_parts(self) -> (String, Option<T>) {
        (self.raw, self.decoded)
    }
}

impl<T> fmt::Debug for DecodedJson<T> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("DecodedJson")
            .field("has_decoded_value", &self.decoded.is_some())
            .finish()
    }
}

impl<'de, T> Deserialize<'de> for DecodedJson<T>
where
    T: DeserializeOwned,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        crate::de::deserialize_decoded_json(deserializer)
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RawFormValue(String);

impl RawFormValue {
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidValue> {
        let value = value.into();
        if value.is_empty() {
            return Err(InvalidValue::new("raw_form_value", "must not be empty"));
        }
        Ok(Self(value))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_accept_strings_and_integers() {
        let string: OrderId = serde_json::from_str(r#""12345""#).unwrap();
        let integer: OrderId = serde_json::from_str("12345").unwrap();
        assert_eq!(string, integer);
        assert_eq!(string.as_str(), "12345");
        assert!(serde_json::from_str::<OrderId>(r#"""#).is_err());
        assert!(serde_json::from_str::<OrderId>("1.5").is_err());
    }

    #[test]
    fn money_accepts_number_or_string_and_serializes_exactly() {
        let string: Money = serde_json::from_str(r#""0.2400""#).unwrap();
        let number: Money = serde_json::from_str("0.24").unwrap();
        assert_eq!(string, number);
        assert_eq!(serde_json::to_string(&string).unwrap(), r#""0.2400""#);
    }

    #[test]
    fn phone_number_accepts_number_but_is_redacted() {
        let phone: PhoneNumber = serde_json::from_str("15551234567").unwrap();
        assert_eq!(phone.expose(), "15551234567");
        assert_eq!(phone.to_string(), "[REDACTED]");
        assert_eq!(format!("{phone:?}"), "PhoneNumber([REDACTED])");
    }

    #[test]
    fn all_sensitive_types_require_explicit_exposure() {
        let sentinel = "sensitive-value-42";
        let values = [
            format!("{:?}", SmsText::new(sentinel).unwrap()),
            format!("{:?}", ActivationToken::new(sentinel).unwrap()),
            format!("{:?}", EsimCredential::new(sentinel).unwrap()),
            format!("{:?}", Password::new(sentinel).unwrap()),
        ];
        assert!(values.iter().all(|value| !value.contains(sentinel)));
        assert_eq!(SmsText::new(sentinel).unwrap().expose(), sentinel);
    }

    #[test]
    fn status_preserves_unknown_shapes() {
        assert_eq!(
            serde_json::from_str::<StatusValue>("7").unwrap(),
            StatusValue::Integer(7)
        );
        assert_eq!(
            serde_json::from_str::<StatusValue>(r#""waiting""#).unwrap(),
            StatusValue::String("waiting".into())
        );
        let other = serde_json::json!({"future": true});
        assert_eq!(
            serde_json::from_value::<StatusValue>(other.clone()).unwrap(),
            StatusValue::Other(other)
        );
    }

    #[test]
    fn decoded_json_debug_omits_raw_and_decoded_content() {
        let sentinel = "private-sms-content";
        let value = DecodedJson::<Value>::from_raw(format!(r#"{{"sms":"{sentinel}"}}"#));
        assert!(value.decoded().is_some());
        assert!(!format!("{value:?}").contains(sentinel));
        assert!(value.raw().contains(sentinel));
    }
}
