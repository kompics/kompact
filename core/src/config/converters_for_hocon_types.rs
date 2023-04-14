use super::*;
use std::time::Duration;

/// Value converter for type `String`
pub struct StringValue;
impl ConfigValueType for StringValue {
    type Value = String;

    fn from_conf(conf: &Hocon) -> Result<Self::Value, ConfigError> {
        conf.as_string()
            .ok_or_else(|| ConfigError::expected::<Self::Value>(conf))
    }

    fn config_string(value: Self::Value) -> String {
        format!(r#""{}""#, value)
    }
}

/// Value converter for type `i64`
pub struct IntegerValue;
impl ConfigValueType for IntegerValue {
    type Value = i64;

    fn from_conf(conf: &Hocon) -> Result<Self::Value, ConfigError> {
        conf.as_i64()
            .ok_or_else(|| ConfigError::expected::<Self::Value>(conf))
    }

    fn config_string(value: Self::Value) -> String {
        format!("{}", value)
    }
}

/// Value converter for type `f64`
pub struct RealValue;
impl ConfigValueType for RealValue {
    type Value = f64;

    fn from_conf(conf: &Hocon) -> Result<Self::Value, ConfigError> {
        conf.as_f64()
            .ok_or_else(|| ConfigError::expected::<Self::Value>(conf))
    }

    fn config_string(value: Self::Value) -> String {
        format!("{}", value)
    }
}

/// Value converter for type `bool`
pub struct BooleanValue;
impl ConfigValueType for BooleanValue {
    type Value = bool;

    fn from_conf(conf: &Hocon) -> Result<Self::Value, ConfigError> {
        conf.as_bool()
            .ok_or_else(|| ConfigError::expected::<Self::Value>(conf))
    }

    fn config_string(value: Self::Value) -> String {
        format!("{}", value)
    }
}

/// Value converter for byte units
pub struct BytesValue;
impl ConfigValueType for BytesValue {
    type Value = u64;

    fn from_conf(conf: &Hocon) -> Result<Self::Value, ConfigError> {
        conf.as_bytes()
            .ok_or_else(|| ConfigError::expected::<Self::Value>(conf))
    }

    fn config_string(value: Self::Value) -> String {
        format!(r#""{}B""#, value)
    }
}

/// Value converter for [Duration](std::time::Duration)
pub struct DurationValue;
impl ConfigValueType for DurationValue {
    type Value = Duration;

    fn from_conf(conf: &Hocon) -> Result<Self::Value, ConfigError> {
        conf.as_duration()
            .ok_or_else(|| ConfigError::expected::<Self::Value>(conf))
    }

    fn config_string(value: Self::Value) -> String {
        format!(r#""{}ms""#, value.as_millis())
    }
}

/// Value converter for arrays of other config values
pub struct ArrayOfValues<T: ConfigValueType> {
    _marker: PhantomData<T>,
}
impl<T: ConfigValueType> Default for ArrayOfValues<T> {
    fn default() -> Self {
        ArrayOfValues {
            _marker: PhantomData,
        }
    }
}
impl<T: ConfigValueType> ConfigValueType for ArrayOfValues<T> {
    type Value = Vec<T::Value>;

    fn from_conf(conf: &Hocon) -> Result<Self::Value, ConfigError> {
        if let Hocon::Array(values) = conf {
            values
                .iter()
                .try_fold(Vec::with_capacity(values.len()), |mut acc, c| {
                    T::from_conf(c).map(|v| {
                        acc.push(v);
                        acc
                    })
                })
        } else {
            Err(ConfigError::expected::<Self::Value>(conf))
        }
    }

    fn config_string(value: Self::Value) -> String {
        let formatted: Vec<String> = value.into_iter().map(T::config_string).collect();
        format!("[{}]", formatted.join(", "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::{conf_test_roundtrip, str_conf};

    #[test]
    fn test_string_roundtrip() {
        conf_test_roundtrip::<StringValue>("test".to_string());
    }

    #[test]
    fn test_integer_roundtrip() {
        conf_test_roundtrip::<IntegerValue>(5i64);
    }

    #[test]
    fn test_real_roundtrip() {
        conf_test_roundtrip::<RealValue>(5.0);
    }

    #[test]
    fn test_boolean_roundtrip() {
        conf_test_roundtrip::<BooleanValue>(true);
    }

    #[test]
    fn test_bytes() {
        let conf = str_conf("size = 1.5KiB");
        let res = BytesValue::from_conf(&conf["size"]);
        assert_eq!(Ok(1536), res);
    }

    #[test]
    fn test_bytes_roundtrip() {
        conf_test_roundtrip::<BytesValue>(1536);
    }

    #[test]
    fn test_duration() {
        let conf = str_conf("time = 3days");
        let res = DurationValue::from_conf(&conf["time"]);
        assert_eq!(Ok(Duration::from_secs(3 * 24 * 60 * 60)), res);
    }

    #[test]
    fn test_duration_roundtrip() {
        conf_test_roundtrip::<DurationValue>(Duration::from_secs(3 * 24 * 60 * 60));
    }

    #[test]
    fn test_array() {
        let conf = str_conf("values = [1, 2, 3, 4, 5]");
        let res = ArrayOfValues::<IntegerValue>::from_conf(&conf["values"]);
        assert_eq!(Ok(vec![1i64, 2i64, 3i64, 4i64, 5i64]), res);
    }

    #[test]
    fn test_array_roundtrip() {
        conf_test_roundtrip::<ArrayOfValues<IntegerValue>>(vec![1i64, 2i64, 3i64, 4i64, 5i64]);
    }
}
