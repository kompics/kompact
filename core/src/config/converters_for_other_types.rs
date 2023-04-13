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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::tests::{conf_test_roundtrip, str_conf};

    #[test]
    fn test_whole_bytes() {
        let conf = str_conf("size = 1.5KiB");
        let res = BytesValue::from_conf(&conf["size"]);
        assert_eq!(Ok(1536u64), res);
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
