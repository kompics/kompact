use std::{
    collections::BTreeMap,
    convert::TryInto,
    error::Error,
    fmt,
    marker::PhantomData,
    ops::Index,
    path::Path,
};

#[macro_use]
mod macros;

mod converters_for_config_values;
pub use converters_for_config_values::*;
mod converters_for_other_types;
pub use converters_for_other_types::*;

const PATH_SEP: char = '.';
const MISSING_KEY: ConfigValue = ConfigValue::BadValue(ConfigPathError::MissingKey);
const INVALID_KEY: ConfigValue = ConfigValue::BadValue(ConfigPathError::InvalidKey);

/// A parsed configuration value stored by Kompact.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigValue {
    /// A string value.
    String(String),
    /// An integer value.
    Integer(i64),
    /// A floating-point value.
    Real(f64),
    /// A boolean value.
    Boolean(bool),
    /// An array value.
    Array(Vec<ConfigValue>),
    /// A hierarchical table value.
    Table(BTreeMap<String, ConfigValue>),
    /// A sentinel used for invalid indexing.
    BadValue(ConfigPathError),
}

impl ConfigValue {
    /// Convert a parsed TOML value into Kompact's config representation.
    pub fn from_toml(value: toml::Value) -> Self {
        match value {
            toml::Value::String(v) => ConfigValue::String(v),
            toml::Value::Integer(v) => ConfigValue::Integer(v),
            toml::Value::Float(v) => ConfigValue::Real(v),
            toml::Value::Boolean(v) => ConfigValue::Boolean(v),
            toml::Value::Array(values) => {
                ConfigValue::Array(values.into_iter().map(ConfigValue::from_toml).collect())
            }
            toml::Value::Table(values) => ConfigValue::Table(
                values
                    .into_iter()
                    .map(|(key, value)| (key, ConfigValue::from_toml(value)))
                    .collect(),
            ),
            toml::Value::Datetime(v) => ConfigValue::String(v.to_string()),
        }
    }

    /// Convert this config value back into a TOML value where possible.
    pub fn to_toml_value(&self) -> Option<toml::Value> {
        match self {
            ConfigValue::String(v) => Some(toml::Value::String(v.clone())),
            ConfigValue::Integer(v) => Some(toml::Value::Integer(*v)),
            ConfigValue::Real(v) => Some(toml::Value::Float(*v)),
            ConfigValue::Boolean(v) => Some(toml::Value::Boolean(*v)),
            ConfigValue::Array(values) => values
                .iter()
                .map(ConfigValue::to_toml_value)
                .collect::<Option<Vec<_>>>()
                .map(toml::Value::Array),
            ConfigValue::Table(values) => values
                .iter()
                .map(|(key, value)| value.to_toml_value().map(|v| (key.clone(), v)))
                .collect::<Option<toml::Table>>()
                .map(toml::Value::Table),
            ConfigValue::BadValue(_) => None,
        }
    }

    /// Render this value as a TOML fragment.
    pub fn to_toml_fragment(&self) -> Option<String> {
        self.to_toml_value().map(|value| value.to_string())
    }

    /// Return the value as a string if possible.
    pub fn as_string(&self) -> Option<String> {
        match self {
            ConfigValue::String(v) => Some(v.clone()),
            _ => None,
        }
    }

    /// Return the value as an integer if possible.
    pub fn as_i64(&self) -> Option<i64> {
        match self {
            ConfigValue::Integer(v) => Some(*v),
            _ => None,
        }
    }

    /// Return the value as a float if possible.
    pub fn as_f64(&self) -> Option<f64> {
        match self {
            ConfigValue::Integer(v) => Some(*v as f64),
            ConfigValue::Real(v) => Some(*v),
            _ => None,
        }
    }

    /// Return the value as a boolean if possible.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            ConfigValue::Boolean(v) => Some(*v),
            _ => None,
        }
    }

    /// Return the value as a byte size if possible.
    pub fn as_bytes(&self) -> Option<u64> {
        match self {
            ConfigValue::Integer(v) if *v >= 0 => Some(*v as u64),
            ConfigValue::String(v) => byte_unit::Byte::parse_str(v, true)
                .ok()
                .map(|bytes| bytes.as_u64()),
            _ => None,
        }
    }

    /// Return the value as a duration if possible.
    pub fn as_duration(&self) -> Option<std::time::Duration> {
        match self {
            ConfigValue::String(v) => humantime::parse_duration(v).ok(),
            _ => None,
        }
    }

    /// Merge another value into this one using recursive table merging and later-value replacement.
    pub fn merge(&mut self, other: ConfigValue) {
        match (self, other) {
            (ConfigValue::Table(current), ConfigValue::Table(next)) => {
                for (key, value) in next {
                    if let Some(existing) = current.get_mut(&key) {
                        existing.merge(value);
                    } else {
                        current.insert(key, value);
                    }
                }
            }
            (current, next) => *current = next,
        }
    }
}

impl Index<&str> for ConfigValue {
    type Output = ConfigValue;

    fn index(&self, key: &str) -> &Self::Output {
        match self {
            ConfigValue::Table(values) => values.get(key).unwrap_or(&MISSING_KEY),
            ConfigValue::BadValue(ConfigPathError::MissingKey) => &MISSING_KEY,
            ConfigValue::BadValue(ConfigPathError::InvalidKey) => &INVALID_KEY,
            _ => &INVALID_KEY,
        }
    }
}

impl Index<usize> for ConfigValue {
    type Output = ConfigValue;

    fn index(&self, index: usize) -> &Self::Output {
        match self {
            ConfigValue::Array(values) => values.get(index).unwrap_or(&MISSING_KEY),
            ConfigValue::BadValue(ConfigPathError::MissingKey) => &MISSING_KEY,
            ConfigValue::BadValue(ConfigPathError::InvalidKey) => &INVALID_KEY,
            _ => &INVALID_KEY,
        }
    }
}

/// Errors raised while traversing a parsed configuration value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConfigPathError {
    /// The requested key or array element was missing.
    MissingKey,
    /// The requested lookup was invalid for the current value type.
    InvalidKey,
}

impl fmt::Display for ConfigPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigPathError::MissingKey => write!(f, "missing key"),
            ConfigPathError::InvalidKey => write!(f, "invalid key"),
        }
    }
}

impl Error for ConfigPathError {}

/// Errors raised while loading TOML configuration sources.
#[derive(Debug)]
pub enum ConfigLoadingError {
    /// Reading a config file failed.
    Io(std::io::Error),
    /// Parsing TOML failed.
    Parse(toml::de::Error),
    /// The top-level TOML value was not a table.
    RootMustBeTable,
}

impl PartialEq for ConfigLoadingError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (ConfigLoadingError::Io(lhs), ConfigLoadingError::Io(rhs)) => lhs.kind() == rhs.kind(),
            (ConfigLoadingError::Parse(lhs), ConfigLoadingError::Parse(rhs)) => {
                lhs.to_string() == rhs.to_string()
            }
            (ConfigLoadingError::RootMustBeTable, ConfigLoadingError::RootMustBeTable) => true,
            _ => false,
        }
    }
}

impl fmt::Display for ConfigLoadingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigLoadingError::Io(err) => write!(f, "failed to read config file: {}", err),
            ConfigLoadingError::Parse(err) => write!(f, "failed to parse TOML config: {}", err),
            ConfigLoadingError::RootMustBeTable => {
                write!(f, "top-level TOML config must be a table")
            }
        }
    }
}

impl Error for ConfigLoadingError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ConfigLoadingError::Io(err) => Some(err),
            ConfigLoadingError::Parse(err) => Some(err),
            ConfigLoadingError::RootMustBeTable => None,
        }
    }
}

impl From<std::io::Error> for ConfigLoadingError {
    fn from(error: std::io::Error) -> Self {
        ConfigLoadingError::Io(error)
    }
}

impl From<toml::de::Error> for ConfigLoadingError {
    fn from(error: toml::de::Error) -> Self {
        ConfigLoadingError::Parse(error)
    }
}

/// Parse a TOML string into a configuration table.
pub fn parse_config_str(config_string: &str) -> Result<ConfigValue, ConfigLoadingError> {
    let config: toml::Value = toml::from_str(config_string)?;
    match ConfigValue::from_toml(config) {
        value @ ConfigValue::Table(_) => Ok(value),
        _ => Err(ConfigLoadingError::RootMustBeTable),
    }
}

/// Parse a TOML file into a configuration table.
pub fn parse_config_file<P>(path: P) -> Result<ConfigValue, ConfigLoadingError>
where
    P: AsRef<Path>,
{
    let config_string = std::fs::read_to_string(path)?;
    parse_config_str(config_string.as_ref())
}

pub(crate) fn insert_config_value(root: &mut ConfigValue, key: &str, value: ConfigValue) {
    let segments: Vec<&str> = key.split(PATH_SEP).collect();
    insert_segments(root, &segments, value);
}

fn insert_segments(current: &mut ConfigValue, segments: &[&str], value: ConfigValue) {
    if segments.is_empty() {
        current.merge(value);
        return;
    }

    if !matches!(current, ConfigValue::Table(_)) {
        *current = ConfigValue::Table(BTreeMap::new());
    }

    let values = match current {
        ConfigValue::Table(values) => values,
        _ => unreachable!("config root should always be a table"),
    };

    if segments.len() == 1 {
        if let Some(existing) = values.get_mut(segments[0]) {
            existing.merge(value);
        } else {
            values.insert(segments[0].to_string(), value);
        }
        return;
    }

    let child = values
        .entry(segments[0].to_string())
        .or_insert_with(|| ConfigValue::Table(BTreeMap::new()));
    insert_segments(child, &segments[1..], value);
}

/// Extension methods for configuration values to support [ConfigEntry](ConfigEntry) lookup.
pub trait ConfigValueExt {
    /// Read the value at the location given by `key` from this config.
    fn get<T>(&self, key: &ConfigEntry<T>) -> Result<T::Value, ConfigError>
    where
        T: ConfigValueType;

    /// Read the value at the location given by `key` from this config, or return the default, if any.
    fn get_or_default<T>(&self, key: &ConfigEntry<T>) -> Result<T::Value, ConfigError>
    where
        T: ConfigValueType;
}

impl ConfigValueExt for ConfigValue {
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

/// Description of a configuration parameter that can be set via TOML config.
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
    /// The full path key to read this config value from a TOML config.
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
    /// Used if no value is specified in the config.
    pub fn default(&self) -> Option<T::Value> {
        self.default.map(|default_fn| default_fn())
    }

    /// Returns all the path segments for the full path for this key up to the root.
    pub fn path_segments(&self) -> Vec<&'static str> {
        self.key.split(PATH_SEP).collect()
    }

    /// Select the entry corresponding to this key from the given config.
    pub fn select<'a>(&self, conf: &'a ConfigValue) -> &'a ConfigValue {
        let path = self.key.split(PATH_SEP);
        let mut value = conf;
        for segment in path {
            value = &value[segment];
        }
        value
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
    pub fn read(&self, conf: &ConfigValue) -> Result<T::Value, ConfigError> {
        let value = self.select(conf);
        if let ConfigValue::BadValue(error) = value {
            Err((*error).into())
        } else {
            let v = T::from_conf(value)?;
            self.validate(v)
        }
    }

    /// Read the value for this key from the given config or return the default value if the path is not present.
    pub fn read_or_default(&self, conf: &ConfigValue) -> Result<T::Value, ConfigError> {
        match self.read(conf) {
            Ok(v) => Ok(v),
            Err(ConfigError::PathError(error)) => match self.default() {
                Some(default) if error == ConfigPathError::MissingKey => Ok(default),
                Some(_) => Err(ConfigError::PathError(error)),
                None => Err(ConfigError::PathError(error)),
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
    fn from_conf(conf: &ConfigValue) -> Result<Self::Value, ConfigError>;

    /// Convert a runtime value into a config value.
    fn into_config_value(value: Self::Value) -> ConfigValue;

    /// Produce a TOML fragment for the value.
    fn config_string(value: Self::Value) -> String {
        Self::into_config_value(value)
            .to_toml_fragment()
            .expect("config values should always serialise to TOML")
    }
}

/// Errors that occur during config lookup.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    /// Type conversion failed.
    ConversionError(String),
    /// Path traversal failed.
    PathError(ConfigPathError),
    /// Value validation failed.
    InvalidValue(String),
}
impl ConfigError {
    fn expected<T>(conf: &ConfigValue) -> Self {
        let descr = format!(
            "Expected {} config value, but got {:?}",
            std::any::type_name::<T>(),
            conf
        );
        ConfigError::ConversionError(descr)
    }
}
impl From<ConfigPathError> for ConfigError {
    fn from(error: ConfigPathError) -> Self {
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
    [kompact]
    my-test-key = "testme"
    my-validate-key = 50

    [kompact.test-group]
    inner-key = "test me inside"
    "#;

    const BAD_CONFIG: &str = r#"
    my-test-key = "testme"

    [kompact]
    my-validate-key = 200
    "#;

    #[test]
    fn simple_config_key() {
        assert_eq!("kompact.my-test-key", SIMPLE_KEY.key);
        assert_eq!(vec!["kompact", "my-test-key"], SIMPLE_KEY.path_segments());

        let conf = parse_config_str(EXAMPLE_CONFIG).expect("config");
        let v = SIMPLE_KEY.read(&conf).expect("String");
        assert_eq!("testme", v);

        let v2 = KEY_FROM_MACRO_NO_DEFAULT.read(&conf).expect("String");
        assert_eq!("test me inside", v2);
    }

    #[test]
    fn default_config_key() {
        let conf = parse_config_str(EXAMPLE_CONFIG).expect("config");
        let v = KEY_WITH_DEFAULT.read_or_default(&conf).expect("String");
        assert_eq!("default string", v);

        let v2 = KEY_FROM_MACRO.read_or_default(&conf).expect("String");
        assert_eq!("default value", v2);
    }

    #[test]
    fn validated_config_key() {
        {
            let conf = parse_config_str(EXAMPLE_CONFIG).expect("config");

            let v = conf.get(&KEY_FROM_MACRO_VALIDATE).unwrap();
            assert_eq!(50, v);
        }
        {
            let conf = parse_config_str(BAD_CONFIG).expect("config");

            let res = conf.get(&KEY_FROM_MACRO_VALIDATE);
            assert!(res.is_err());
        }
    }

    #[test]
    fn simple_key_bad_config() {
        let conf = parse_config_str(BAD_CONFIG).expect("config");
        let res = SIMPLE_KEY.read(&conf);
        assert_eq!(
            Err(ConfigError::PathError(ConfigPathError::MissingKey)),
            res
        );
    }

    pub(super) fn str_conf(config_string: &str) -> ConfigValue {
        parse_config_str(config_string).expect("config")
    }

    const ROUNDTRIP_KEY: &str = "value";

    pub(super) fn conf_test_roundtrip<T: ConfigValueType>(value: T::Value)
    where
        T::Value: PartialEq + fmt::Debug + Clone,
    {
        let config_string = format!("{} = {}", ROUNDTRIP_KEY, T::config_string(value.clone()));
        let conf = parse_config_str(config_string.as_ref()).expect("config");
        let res = T::from_conf(&conf[ROUNDTRIP_KEY]);
        assert_eq!(Ok(value), res);
    }
}
