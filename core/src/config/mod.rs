use hocon::Hocon;
use std::{error::Error, fmt, marker::PhantomData};

#[macro_use]
mod macros;

/// Top level Kompact configuration keys.
pub mod keys {
    //use crate::kompact_config_group;

    // kompact_config_group!(
    //     KOMPACT,
    //     key = "kompact",
    //     doc = "The top-level group for kompact configuration items.",
    //     version = "0.11"
    // );

    // kompact_config_group!(
    //     TEST,
    //     key = "test",
    //     parent = crate::config::keys::KOMPACT,
    //     doc = "Test group",
    //     version = "0.11");
}

const PATH_SEP: &str = ".";

/// A group of config values.
///
/// This corresponds to one of the path elements of non-leaf HOCON node.
///
/// # Example
///
/// For a HOCON path `kompact.net.my-key` both `kompact` and `net` are groups,
/// while `my-key` is a leaf node and corresponds to a [ConfigEntry](ConfigEntry).
pub struct ConfigGroup {
    /// The key for this group.
    key: &'static str,
    /// An optional parent group.
    parent: Option<&'static ConfigGroup>,
    /// Documentation for this config group.
    doc: &'static str,
    /// The Kompact version in which the group was introduced.
    version: &'static str,
}

impl ConfigGroup {
    /// Returns all the path segments for the full path for this key up to the root.
    ///
    /// #Note
    ///
    /// Path segments are returned in reverse order, such that popping
    /// from the vector produces the root first and the current last.
    pub fn path_segments(&self) -> Vec<&'static str> {
        let mut path = vec![self.key];
        let mut group: Option<&'static ConfigGroup> = self.parent;
        while group.is_some() {
            let parent = group.unwrap();
            path.push(parent.key);
            group = parent.parent;
        }
        path
    }

    /// Returns the full path for this key up to the root.
    pub fn path(&self) -> String {
        let mut path = self.path_segments();
        path.reverse();
        path.join(PATH_SEP)
    }
}

/// Description of a configuration parameter that can be set via HOCON config.
pub struct ConfigEntry<T>
where
    T: ConfigValueType,
{
    /// The key to read this config value from a HOCON config.
    key: &'static str,
    /// The group this config key belong to.
    ///
    /// All config entries should belong to exactly one group.
    /// Top level keys should belong to the [`kompact` group](keys::KOMPACT).
    parent: &'static ConfigGroup,
    /// Documentation for this config entry.
    doc: &'static str,
    /// The Kompact version in which the value was introduced.
    version: &'static str,
    /// Type information of this config value.
    ///
    /// Can be used for converting the raw config value into a runtime type.
    value_type: PhantomData<T>,
    /// The default value for this config entry.
    ///
    /// Used if no value is specified in the Hocon config.
    default: Option<fn() -> T::Value>,
}

impl<T> ConfigEntry<T>
where
    T: ConfigValueType,
{
    /// Returns all the path segments for the full path for this key up to the root.
    ///
    /// #Note
    ///
    /// Path segments are returned in reverse order, such that popping
    /// from the vector produces the root first and the leaf last.
    pub fn path_segments(&self) -> Vec<&'static str> {
        let mut path = vec![self.key];
        let mut group: Option<&'static ConfigGroup> = Some(self.parent);
        while group.is_some() {
            let parent = group.unwrap();
            path.push(parent.key);
            group = parent.parent;
        }
        path
    }

    /// Returns the full path for this key up to the root.
    pub fn path(&self) -> String {
        let mut path = self.path_segments();
        path.reverse();
        path.join(PATH_SEP)
    }

    /// Select the entry corresponding to this key from the given config.
    pub fn select<'a>(&self, conf: &'a Hocon) -> &'a Hocon {
        let mut path = self.path_segments();
        let mut hocon = conf;
        while let Some(segment) = path.pop() {
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

    // const SIMPLE_KEY: ConfigEntry<StringValue> = ConfigEntry {
    //     key: "my-test-key",
    //     parent: &keys::KOMPACT,
    //     doc: "This a simple test key for String value.",
    //     version: "0.11",
    //     value_type: PhantomData,
    //     default: None,
    // };

    // kompact_config_group!(
    //     TEST_GROUP,
    //     key = "test-group",
    //     parent = keys::KOMPACT,
    //     doc = "My test documentation",
    //     version = "0.11"
    // );

    const EXAMPLE_CONFIG: &str = r#"
    kompact {
        my-test-key: "testme",

        test-group {
            inner-key: "testme"
        }
    }
    "#;

    const BAD_CONFIG: &str = r#"
    my-test-key: "testme"
    "#;

    // #[test]
    // fn simple_config_key() {
    //     assert_eq!("my-test-key", SIMPLE_KEY.key);

    //     assert_eq!("kompact.my-test-key", SIMPLE_KEY.path());

    //     let loader = HoconLoader::new().load_str(EXAMPLE_CONFIG).expect("config");
    //     let hocon = loader.hocon().expect("config");
    //     let v = SIMPLE_KEY.read(&hocon).expect("String");
    //     assert_eq!("testme", v);
    // }

    // #[test]
    // fn nested_config_key() {
    //     assert_eq!("test-group", TEST_GROUP.key);

    //     assert_eq!("kompact.test-group", TEST_GROUP.path());

    //     // let loader = HoconLoader::new().load_str(EXAMPLE_CONFIG).expect("config");
    //     // let hocon = loader.hocon().expect("config");
    //     // let v = SIMPLE_KEY.read(&hocon).expect("String");
    //     // assert_eq!("testme", v);
    // }

    // #[test]
    // fn simple_key_bad_config() {
    //     let loader = HoconLoader::new().load_str(BAD_CONFIG).expect("config");
    //     let hocon = loader.hocon().expect("config");
    //     let res = SIMPLE_KEY.read(&hocon);
    //     assert_eq!(Err(ConfigError::PathError(hocon::Error::InvalidKey)), res);
    // }
}
