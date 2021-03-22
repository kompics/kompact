use super::*;

/// Value converter for type `usize`
pub struct UsizeValue;
impl ConfigValueType for UsizeValue {
    type Value = usize;

    fn from_conf(conf: &Hocon) -> Result<Self::Value, ConfigError> {
        let res = conf
            .as_i64()
            .ok_or_else(|| ConfigError::expected::<Self::Value>(conf))?;
        let ures: usize = res.try_into()?;
        Ok(ures)
    }

    fn config_string(value: Self::Value) -> String {
        format!("{}", value)
    }
}

/// Value converter for type `f32`
pub struct F32Value;
impl ConfigValueType for F32Value {
    type Value = f32;

    fn from_conf(conf: &Hocon) -> Result<Self::Value, ConfigError> {
        conf.as_f64()
            .map(|v| v as f32) // this is safe...only loses accuracy
            .ok_or_else(|| ConfigError::expected::<Self::Value>(conf))
    }

    fn config_string(value: Self::Value) -> String {
        format!("{}", value)
    }
}

/// Value converter for byte units
///
/// This is the same as [BytesValues](crate::config::BytesValue)
/// expect it only accepts positive values and rounds to the next integer.
pub struct WholeBytesValue;
impl ConfigValueType for WholeBytesValue {
    type Value = u64;

    fn from_conf(conf: &Hocon) -> Result<Self::Value, ConfigError> {
        let res = conf
            .as_bytes()
            .ok_or_else(|| ConfigError::expected::<f64>(conf))?;
        config_assert!(res > 0.0, res);
        let rounded = res.round() as u64;
        Ok(rounded)
    }

    fn config_string(value: Self::Value) -> String {
        BytesValue::config_string(value as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::{conf_test_roundtrip, str_conf};

    #[test]
    fn test_whole_bytes() {
        let conf = str_conf("size = 1.5KiB");
        let res = WholeBytesValue::from_conf(&conf["size"]);
        assert_eq!(Ok(1536u64), res);
    }

    #[test]
    fn test_whole_bytes_error() {
        let conf = str_conf("size = -1.5KiB");
        let res = WholeBytesValue::from_conf(&conf["size"]);
        assert!(res.is_err());
        println!("WholeBytesValue error message: {}", res.unwrap_err());
    }

    #[test]
    fn test_whole_bytes_roundtrip() {
        conf_test_roundtrip::<WholeBytesValue>(1536);
    }

    #[test]
    fn test_usize_roundtrip() {
        conf_test_roundtrip::<UsizeValue>(1536usize);
    }

    #[test]
    fn test_f32_roundtrip() {
        conf_test_roundtrip::<F32Value>(5.0f32);
    }
}
