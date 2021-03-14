use hocon::Hocon;
use std::{convert::TryInto, error::Error, fmt, marker::PhantomData};

#[macro_use]
mod macros;

const PATH_SEP: char = '.';

/// Extension methods for Hocon instances to support [ConfigEntry](ConfigEntry) lookup.
pub trait HoconExt {
    /// Read the value at the location given by `key` from this config.
    fn get<T>(&self, key: &ConfigEntry<T>) -> Result<T::Value, ConfigError>
    where
        T: ConfigValueType;

    /// Read the value at the location given by `key` from this config, or return the default, if any.
    fn get_or_default<T>(&self, key: &ConfigEntry<T>) -> Result<T::Value, ConfigError>
    where
        T: ConfigValueType;
}

impl HoconExt for Hocon {
    fn get<T>(&self, key: &ConfigEntry<T>) -> Result<T::Value, ConfigError>
    where
        T: ConfigValueType,
    {
        key.read(self)
    }

    fn get_or_default<T>(&self, key: &ConfigEntry<T>) -> Result<T::Value, ConfigError>
    where
        T: ConfigValueType,
    {
        key.read_or_default(self)
    }
}

/// A validator for extracted values of `T::Value`
pub type ValidatorFun<T> = fn(&<T as ConfigValueType>::Value) -> Result<(), String>;

/// Description of a configuration parameter that can be set via HOCON config.
///
/// - `T`: Type information of this config value. Can be used for converting the raw config value into a runtime type.
///
/// # Note
///
/// This should be created via the [kompact_config](crate::kompact_config) macro and not directly.
#[derive(Clone)]
pub struct ConfigEntry<T>
where
    T: ConfigValueType,
{
    /// The full path key to read this config value from a HOCON config.
    pub key: &'static str,
    /// Documentation for this config entry.
    pub doc: &'static str,
    /// The Kompact version in which the value was introduced.
    pub version: &'static str,
    #[doc(hidden)]
    pub value_type: PhantomData<T>,
    #[doc(hidden)]
    pub default: Option<fn() -> T::Value>,
    #[doc(hidden)]
    pub validator: Option<ValidatorFun<T>>,
}

impl<T> ConfigEntry<T>
where
    T: ConfigValueType,
{
    /// The default value for this config entry.
    ///
    /// Used if no value is specified in the Hocon config.
    pub fn default(&self) -> Option<T::Value> {
        self.default.map(|default_fn| default_fn())
    }

    /// Returns all the path segments for the full path for this key up to the root.
    pub fn path_segments(&self) -> Vec<&'static str> {
        self.key.split(PATH_SEP).collect()
    }

    /// Select the entry corresponding to this key from the given config.
    pub fn select<'a>(&self, conf: &'a Hocon) -> &'a Hocon {
        let path = self.key.split(PATH_SEP);
        let mut hocon = conf;
        for segment in path {
            hocon = &hocon[segment];
        }
        hocon
    }

    /// Performs the validation of the given value
    ///
    /// If no validator is specified, the value is simply passed through.
    pub fn validate(&self, v: T::Value) -> Result<T::Value, ConfigError> {
        if let Some(validator) = self.validator {
            match validator(&v) {
                Ok(_) => Ok(v),
                Err(err_msg) => Err(ConfigError::InvalidValue(err_msg)),
            }
        } else {
            Ok(v)
        }
    }

    /// Read the value for this key from the given config.
    pub fn read(&self, conf: &Hocon) -> Result<T::Value, ConfigError> {
        let hocon = self.select(conf);
        if let Hocon::BadValue(error) = hocon {
            Err(error.clone().into())
        } else {
            let v = T::from_conf(hocon)?;
            self.validate(v)
        }
    }

    /// Read the value for this key from the given config or return the default value if the path is not present.
    pub fn read_or_default(&self, conf: &Hocon) -> Result<T::Value, ConfigError> {
        match self.read(conf) {
            Ok(v) => Ok(v),
            Err(ConfigError::PathError(e)) => match self.default() {
                Some(default) => Ok(default),
                None => Err(ConfigError::PathError(e)),
            },
            Err(e) => Err(e),
        }
    }
}

/// A value extractor for config values
pub trait ConfigValueType {
    /// The type of the value extracted by this type.
    type Value;

    /// Extract the value from a config instance.
    fn from_conf(conf: &Hocon) -> Result<Self::Value, ConfigError>;

    /// Produce a format for the value that can be spliced into a config string.
    fn config_string(value: Self::Value) -> String;
}

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

/// Errors that occur during config lookup.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    /// Type conversion failed.
    ConversionError(String),
    /// Path traversal failed.
    PathError(hocon::Error),
    /// Value validation failed.
    InvalidValue(String),
}
impl ConfigError {
    fn expected<T>(conf: &Hocon) -> Self {
        let descr = format!(
            "Expected {} config value, but got {:?}",
            std::any::type_name::<T>(),
            conf
        );
        ConfigError::ConversionError(descr)
    }
}
impl From<hocon::Error> for ConfigError {
    fn from(error: hocon::Error) -> Self {
        ConfigError::PathError(error)
    }
}
impl From<std::num::TryFromIntError> for ConfigError {
    fn from(error: std::num::TryFromIntError) -> Self {
        ConfigError::ConversionError(format!("{}", error))
    }
}
impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::ConversionError(description) => {
                write!(f, "Error during type conversion: {}", description)
            }
            ConfigError::PathError(error) => write!(f, "Error during path traversal: {}", error),
            ConfigError::InvalidValue(description) => {
                write!(f, "Error during value validation: {}", description)
            }
        }
    }
}
impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ConfigError::ConversionError(_) => None,
            ConfigError::PathError(error) => Some(error),
            ConfigError::InvalidValue(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hocon::HoconLoader;

    const SIMPLE_KEY: ConfigEntry<StringValue> = ConfigEntry {
        key: "kompact.my-test-key",
        doc: "This a simple test key for String value.",
        version: "0.11",
        value_type: PhantomData,
        default: None,
        validator: None,
    };

    const KEY_WITH_DEFAULT: ConfigEntry<StringValue> = {
        fn default_value() -> String {
            String::from("default string")
        }

        ConfigEntry {
            key: "kompact.my-default-key",
            doc: "This a simple test key for String value.",
            version: "0.11",
            value_type: PhantomData,
            default: Some(default_value),
            validator: None,
        }
    };

    kompact_config! {
        KEY_FROM_MACRO,
        key = "kompact.my-macro-key",
        type = StringValue,
        default = String::from("default value"),
        doc = "A config key generated from a macro.",
        version = "0.11"
    }

    kompact_config! {
        KEY_FROM_MACRO_NO_DEFAULT,
        key = "kompact.test-group.inner-key",
        doc = "A config key generated from a macro.",
        version = "0.11"
    }

    kompact_config! {
        KEY_FROM_MACRO_VALIDATE,
        key = "kompact.my-validate-key",
        type = UsizeValue,
        default = 0,
        validate = |value| *value < 100,
        doc = "A config key generated from a macro with a validator.",
        version = "0.11"
    }

    const EXAMPLE_CONFIG: &str = r#"
    kompact {
        my-test-key = "testme",

        my-validate-key = 50

        test-group {
            inner-key = "test me inside"
        }
    }
    "#;

    const BAD_CONFIG: &str = r#"
    my-test-key = "testme"
    kompact.my-validate-key = 200
    "#;

    #[test]
    fn simple_config_key() {
        assert_eq!("kompact.my-test-key", SIMPLE_KEY.key);
        assert_eq!(vec!["kompact", "my-test-key"], SIMPLE_KEY.path_segments());

        let loader = HoconLoader::new().load_str(EXAMPLE_CONFIG).expect("config");
        let hocon = loader.hocon().expect("config");
        let v = SIMPLE_KEY.read(&hocon).expect("String");
        assert_eq!("testme", v);

        let v2 = KEY_FROM_MACRO_NO_DEFAULT.read(&hocon).expect("String");
        assert_eq!("test me inside", v2);
    }

    #[test]
    fn default_config_key() {
        let loader = HoconLoader::new().load_str(EXAMPLE_CONFIG).expect("config");
        let hocon = loader.hocon().expect("config");
        let v = KEY_WITH_DEFAULT.read_or_default(&hocon).expect("String");
        assert_eq!("default string", v);

        let v2 = KEY_FROM_MACRO.read_or_default(&hocon).expect("String");
        assert_eq!("default value", v2);
    }

    #[test]
    fn validated_config_key() {
        {
            let loader = HoconLoader::new().load_str(EXAMPLE_CONFIG).expect("config");
            let hocon = loader.hocon().expect("config");

            let v = hocon.get(&KEY_FROM_MACRO_VALIDATE).unwrap();
            assert_eq!(50, v);
        }
        {
            let loader = HoconLoader::new().load_str(BAD_CONFIG).expect("config");
            let hocon = loader.hocon().expect("config");

            let res = hocon.get(&KEY_FROM_MACRO_VALIDATE);
            assert!(res.is_err());
        }
    }

    #[test]
    fn simple_key_bad_config() {
        let loader = HoconLoader::new().load_str(BAD_CONFIG).expect("config");
        let hocon = loader.hocon().expect("config");
        let res = SIMPLE_KEY.read(&hocon);
        assert_eq!(Err(ConfigError::PathError(hocon::Error::MissingKey)), res);
    }
}
