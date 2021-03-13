use hocon::Hocon;
use std::{error::Error, fmt, marker::PhantomData};

#[macro_use]
mod macros;

const PATH_SEP: char = '.';

kompact_config! {
    TEST_KEY,
    key = "kompact.test.key",
    doc = r#"A test key.

This can be used to define additional comment options.

# Note

Formatting should work for keys as well, I hope.
    "#,
    version = "0.11"
}

/// Extension methods for Hocon instances to support [ConfigEntry](ConfigEntry) lookup.
pub trait HoconExt {
    /// Read the value at the location given by `key` from this config.
    fn get<T>(&self, key: &ConfigEntry<T>) -> Result<T::Value, ConfigError> where T: ConfigValueType;

    /// Read the value at the location given by `key` from this config, or return the default, if any.
    fn get_or_default<T>(&self, key: &ConfigEntry<T>) -> Result<T::Value, ConfigError> where T: ConfigValueType;
}

impl HoconExt for Hocon {
    fn get<T>(&self, key: &ConfigEntry<T>) -> Result<T::Value, ConfigError> where T: ConfigValueType {
        key.read(self)
    }
    fn get_or_default<T>(&self, key: &ConfigEntry<T>) -> Result<T::Value, ConfigError> where T: ConfigValueType {
        key.read_or_default(self)
    }
}

/// Description of a configuration parameter that can be set via HOCON config.
///
/// - `T`: Type information of this config value. Can be used for converting the raw config value into a runtime type.
///
/// # Note
///
/// This should be created via the [kompact_config](crate::kompact_config) macro and not directly.
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
    value_type: PhantomData<T>,
    default: Option<fn() -> T::Value>,
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

    /// Read the value for this key from the given config.
    pub fn read(&self, conf: &Hocon) -> Result<T::Value, ConfigError> {
        let hocon = self.select(conf);
        if let Hocon::BadValue(error) = hocon {
            Err(error.clone().into())
        } else {
            T::from_conf(hocon)
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

    // TODO: Hocon has no mutable indexing support, so we gotta traverse the tree
    // pub fn write(&self, conf: &mut Hocon, value: T::Value) {

    // }
}

/// A value extractor for config values
pub trait ConfigValueType {
    /// The type of the value extracted by this type.
    type Value;

    /// Extract the value from a config instance.
    fn from_conf(conf: &Hocon) -> Result<Self::Value, ConfigError>;
}

/// Value converter for type `String`
pub struct StringValue;
impl ConfigValueType for StringValue {
    type Value = String;

    fn from_conf(conf: &Hocon) -> Result<Self::Value, ConfigError> {
        conf.as_string()
            .ok_or_else(|| ConfigError::expected::<Self::Value>(conf))
    }
}

/// Errors that occur during config lookup.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    /// Type conversion failed.
    ConversionError(String),
    /// Path traversal failed.
    PathError(hocon::Error),
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
impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::ConversionError(description) => {
                write!(f, "Error during type conversion: {}", description)
            }
            ConfigError::PathError(error) => write!(f, "Error during path traversal: {}", error),
        }
    }
}
impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ConfigError::ConversionError(_) => None,
            ConfigError::PathError(error) => Some(error),
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

    const EXAMPLE_CONFIG: &str = r#"
    kompact {
        my-test-key: "testme",

        test-group {
            inner-key: "test me inside"
        }
    }
    "#;

    const BAD_CONFIG: &str = r#"
    my-test-key: "testme"
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

    // #[test]
    // fn nested_config_key() {
    //     assert_eq!("test-group", TEST_GROUP.key);

    //     assert_eq!("kompact.test-group", TEST_GROUP.path());

    //     // let loader = HoconLoader::new().load_str(EXAMPLE_CONFIG).expect("config");
    //     // let hocon = loader.hocon().expect("config");
    //     // let v = SIMPLE_KEY.read(&hocon).expect("String");
    //     // assert_eq!("testme", v);
    // }

    #[test]
    fn simple_key_bad_config() {
        let loader = HoconLoader::new().load_str(BAD_CONFIG).expect("config");
        let hocon = loader.hocon().expect("config");
        let res = SIMPLE_KEY.read(&hocon);
        assert_eq!(Err(ConfigError::PathError(hocon::Error::InvalidKey)), res);
    }
}
