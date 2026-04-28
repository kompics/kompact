use std::{
    collections::HashMap,
    convert::TryInto,
    error::Error,
    fmt,
    iter::FusedIterator,
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

type ConfigTable = HashMap<String, ConfigValue>;

/// A parsed Kompact configuration document.
///
/// The fallible lookup API is exposed via [Config::get] and [Config::select].
/// For convenience, this type also supports panicking indexing with `[]`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Config {
    root: ConfigTable,
}

impl Config {
    /// Create an empty configuration document.
    pub fn new() -> Self {
        Self::default()
    }

    fn from_toml_table(table: toml::Table) -> Self {
        Config {
            root: table
                .into_iter()
                .map(|(key, value)| (key, ConfigValue::from_toml(value)))
                .collect(),
        }
    }

    pub(crate) fn merge(&mut self, other: Config) {
        merge_tables(&mut self.root, other.root);
    }

    /// Select a single top-level key from the configuration.
    pub fn get<'config, 'path>(&'config self, key: &'path str) -> ConfigLookup<'config, 'path> {
        match self.root.get(key) {
            Some(value) => ConfigLookup::from_full_path(key, value),
            None => ConfigLookup::error_path(key, LookupErrorKind::MissingKey),
        }
    }

    /// Select a dotted path from the configuration.
    pub fn select<'config, 'path>(&'config self, path: &'path str) -> ConfigLookup<'config, 'path> {
        let mut segments = path.split(PATH_SEP);
        let Some(first) = segments.next() else {
            return ConfigLookup::error_path(path, LookupErrorKind::InvalidPath);
        };
        if first.is_empty() {
            return ConfigLookup::error_path(path, LookupErrorKind::InvalidPath);
        }

        let Some(mut value) = self.root.get(first) else {
            return ConfigLookup::error_path(path, LookupErrorKind::MissingKey);
        };
        for segment in segments {
            if segment.is_empty() {
                return ConfigLookup::error_path(path, LookupErrorKind::InvalidPath);
            }
            value = match &value.inner {
                ConfigValueInner::Table(values) => match values.get(segment) {
                    Some(value) => value,
                    None => return ConfigLookup::error_path(path, LookupErrorKind::MissingKey),
                },
                _ => {
                    return ConfigLookup::error_path(
                        path,
                        LookupErrorKind::InvalidKeyAccess {
                            key: segment,
                            value_type: value.value_type_name(),
                        },
                    );
                }
            };
        }
        ConfigLookup::from_full_path(path, value)
    }

    /// Read a typed config entry from this config.
    pub fn read<T>(&self, key: &ConfigEntry<T>) -> Result<T::Value, ConfigError>
    where
        T: ConfigValueType,
    {
        key.read(self)
    }

    /// Read a typed config entry from this config, or fall back to its default value.
    pub fn read_or_default<T>(&self, key: &ConfigEntry<T>) -> Result<T::Value, ConfigError>
    where
        T: ConfigValueType,
    {
        key.read_or_default(self)
    }

    pub(crate) fn insert_value(&mut self, key: &str, value: ConfigValue) {
        let segments: Vec<&str> = key.split(PATH_SEP).collect();
        insert_segments(&mut self.root, &segments, value);
    }
}

impl Index<&str> for Config {
    type Output = ConfigValue;

    fn index(&self, key: &str) -> &Self::Output {
        self.root
            .get(key)
            .unwrap_or_else(|| panic!("missing config key `{}`", key))
    }
}

/// A parsed configuration value.
///
/// Use [Config::get] or [Config::select] for path-aware fallible lookups with detailed
/// error messages. The indexing API on this type is intentionally panicking and is meant
/// only as a convenience shorthand.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigValue {
    pub(crate) inner: ConfigValueInner,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ConfigValueInner {
    String(String),
    Integer(i64),
    Real(f64),
    Boolean(bool),
    DateTime(toml::value::Datetime),
    Array(Vec<ConfigValue>),
    Table(ConfigTable),
}

impl ConfigValue {
    pub(crate) fn string(value: String) -> Self {
        ConfigValue {
            inner: ConfigValueInner::String(value),
        }
    }

    pub(crate) fn integer(value: i64) -> Self {
        ConfigValue {
            inner: ConfigValueInner::Integer(value),
        }
    }

    pub(crate) fn real(value: f64) -> Self {
        ConfigValue {
            inner: ConfigValueInner::Real(value),
        }
    }

    pub(crate) fn boolean(value: bool) -> Self {
        ConfigValue {
            inner: ConfigValueInner::Boolean(value),
        }
    }

    pub(crate) fn datetime(value: toml::value::Datetime) -> Self {
        ConfigValue {
            inner: ConfigValueInner::DateTime(value),
        }
    }

    pub(crate) fn array(values: Vec<ConfigValue>) -> Self {
        ConfigValue {
            inner: ConfigValueInner::Array(values),
        }
    }

    pub(crate) fn table() -> Self {
        ConfigValue {
            inner: ConfigValueInner::Table(HashMap::new()),
        }
    }

    fn from_toml(value: toml::Value) -> Self {
        match value {
            toml::Value::String(v) => ConfigValue::string(v),
            toml::Value::Integer(v) => ConfigValue::integer(v),
            toml::Value::Float(v) => ConfigValue::real(v),
            toml::Value::Boolean(v) => ConfigValue::boolean(v),
            toml::Value::Datetime(v) => ConfigValue::datetime(v),
            toml::Value::Array(values) => {
                ConfigValue::array(values.into_iter().map(ConfigValue::from_toml).collect())
            }
            toml::Value::Table(values) => ConfigValue {
                inner: ConfigValueInner::Table(
                    values
                        .into_iter()
                        .map(|(key, value)| (key, ConfigValue::from_toml(value)))
                        .collect(),
                ),
            },
        }
    }

    pub(crate) fn to_toml_value(&self) -> toml::Value {
        match &self.inner {
            ConfigValueInner::String(v) => toml::Value::String(v.clone()),
            ConfigValueInner::Integer(v) => toml::Value::Integer(*v),
            ConfigValueInner::Real(v) => toml::Value::Float(*v),
            ConfigValueInner::Boolean(v) => toml::Value::Boolean(*v),
            ConfigValueInner::DateTime(v) => toml::Value::Datetime(*v),
            ConfigValueInner::Array(values) => {
                toml::Value::Array(values.iter().map(ConfigValue::to_toml_value).collect())
            }
            ConfigValueInner::Table(values) => toml::Value::Table(
                values
                    .iter()
                    .map(|(key, value)| (key.clone(), value.to_toml_value()))
                    .collect(),
            ),
        }
    }

    /// Render this value as a TOML fragment.
    pub fn to_toml_fragment(&self) -> String {
        self.to_toml_value().to_string()
    }

    /// Return the value as a string if possible.
    pub fn as_string(&self) -> Option<String> {
        match &self.inner {
            ConfigValueInner::String(v) => Some(v.clone()),
            _ => None,
        }
    }

    /// Return the value as an integer if possible.
    pub fn as_i64(&self) -> Option<i64> {
        match &self.inner {
            ConfigValueInner::Integer(v) => Some(*v),
            _ => None,
        }
    }

    /// Return the value as a float if possible.
    pub fn as_f64(&self) -> Option<f64> {
        match &self.inner {
            ConfigValueInner::Integer(v) => Some(*v as f64),
            ConfigValueInner::Real(v) => Some(*v),
            _ => None,
        }
    }

    /// Return the value as a boolean if possible.
    pub fn as_bool(&self) -> Option<bool> {
        match &self.inner {
            ConfigValueInner::Boolean(v) => Some(*v),
            _ => None,
        }
    }

    /// Return the value as a TOML datetime if possible.
    pub fn as_datetime(&self) -> Option<&toml::value::Datetime> {
        match &self.inner {
            ConfigValueInner::DateTime(v) => Some(v),
            _ => None,
        }
    }

    /// Return the value as a byte size if possible.
    pub fn as_bytes(&self) -> Option<u64> {
        match &self.inner {
            ConfigValueInner::Integer(v) if *v >= 0 => Some(*v as u64),
            ConfigValueInner::String(v) => byte_unit::Byte::parse_str(v, true)
                .ok()
                .map(|bytes| bytes.as_u64()),
            _ => None,
        }
    }

    /// Return the value as a duration if possible.
    pub fn as_duration(&self) -> Option<std::time::Duration> {
        match &self.inner {
            ConfigValueInner::String(v) => humantime::parse_duration(v).ok(),
            _ => None,
        }
    }

    /// Return the value as an array slice if possible.
    ///
    /// This is useful when callers need to discover which array indices are
    /// present instead of probing individual indices with [Index].
    ///
    /// ```
    /// use kompact::config::parse_config_str;
    ///
    /// let conf = parse_config_str("values = [1, 2, 3]").expect("config");
    /// let values = conf["values"].as_array().expect("array");
    ///
    /// assert_eq!(3, values.len());
    /// assert_eq!(Some(2), values[1].as_i64());
    /// ```
    pub fn as_array(&self) -> Option<&[ConfigValue]> {
        match &self.inner {
            ConfigValueInner::Array(values) => Some(values.as_slice()),
            _ => None,
        }
    }

    fn value_type_name(&self) -> &'static str {
        match &self.inner {
            ConfigValueInner::String(_) => "string",
            ConfigValueInner::Integer(_) => "integer",
            ConfigValueInner::Real(_) => "float",
            ConfigValueInner::Boolean(_) => "boolean",
            ConfigValueInner::DateTime(_) => "datetime",
            ConfigValueInner::Array(_) => "array",
            ConfigValueInner::Table(_) => "table",
        }
    }

    fn merge(&mut self, other: ConfigValue) {
        match (&mut self.inner, other.inner) {
            (ConfigValueInner::Table(current), ConfigValueInner::Table(next)) => {
                merge_tables(current, next);
            }
            (current, next) => *current = next,
        }
    }
}

impl Index<&str> for ConfigValue {
    type Output = ConfigValue;

    fn index(&self, key: &str) -> &Self::Output {
        match &self.inner {
            ConfigValueInner::Table(values) => values
                .get(key)
                .unwrap_or_else(|| panic!("missing config key `{}`", key)),
            _ => panic!(
                "cannot index config key `{}` into {} value",
                key,
                self.value_type_name()
            ),
        }
    }
}

impl Index<usize> for ConfigValue {
    type Output = ConfigValue;

    fn index(&self, index: usize) -> &Self::Output {
        match &self.inner {
            ConfigValueInner::Array(values) => values.get(index).unwrap_or_else(|| {
                panic!(
                    "index out of bounds: the len is {} but the index is {}",
                    values.len(),
                    index
                )
            }),
            _ => panic!(
                "cannot index config index [{}] into {} value",
                index,
                self.value_type_name()
            ),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum PathSegment<'path> {
    Key(&'path str),
    Index(usize),
}

impl fmt::Display for PathSegment<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PathSegment::Key(key) => write!(f, "{}", key),
            PathSegment::Index(index) => write!(f, "[{}]", index),
        }
    }
}

#[derive(Clone, Copy)]
enum LookupPathSource<'path> {
    FullPath(&'path str),
    Child {
        parent: &'path dyn fmt::Display,
        segment: PathSegment<'path>,
        prepend_separator: bool,
    },
}

impl fmt::Display for LookupPathSource<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LookupPathSource::FullPath(path) => write!(f, "{}", path),
            LookupPathSource::Child {
                parent,
                segment,
                prepend_separator,
            } => {
                write!(f, "{}", parent)?;
                if *prepend_separator {
                    write!(f, "{}", PATH_SEP)?;
                }
                write!(f, "{}", segment)
            }
        }
    }
}

impl fmt::Debug for LookupPathSource<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "LookupPathSource({})", self)
    }
}

#[derive(Debug, Clone, Copy)]
enum LookupErrorKind<'path> {
    InvalidPath,
    MissingKey,
    MissingIndex,
    InvalidKeyAccess {
        key: &'path str,
        value_type: &'static str,
    },
    InvalidIndexAccess {
        index: usize,
        value_type: &'static str,
    },
}

/// A fallible config path lookup that carries the traversed path for later conversions.
///
/// - `'config`: lifetime of the selected value borrowed from the underlying config tree.
/// - `'path`: lifetime of any borrowed path fragments and the parent lookup chain that are
///   used to render detailed path errors lazily.
#[derive(Debug, Clone, Copy)]
pub struct ConfigLookup<'config, 'path> {
    inner: ConfigLookupInner<'config, 'path>,
}

/// Iterator over array elements selected by a [ConfigLookup].
///
/// Each item contains the element index and a path-aware lookup for the
/// corresponding element.
#[derive(Debug, Clone)]
pub struct ConfigArrayIter<'config, 'lookup, 'path> {
    parent: &'lookup ConfigLookup<'config, 'path>,
    values: std::iter::Enumerate<std::slice::Iter<'config, ConfigValue>>,
}

impl<'config, 'lookup, 'path> Iterator for ConfigArrayIter<'config, 'lookup, 'path> {
    type Item = (usize, ConfigLookup<'config, 'lookup>);

    fn next(&mut self) -> Option<Self::Item> {
        self.values.next().map(|(index, value)| {
            let path = LookupPathSource::Child {
                parent: self.parent,
                segment: PathSegment::Index(index),
                prepend_separator: false,
            };
            (
                index,
                ConfigLookup {
                    inner: ConfigLookupInner::Value { value, path },
                },
            )
        })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.values.size_hint()
    }
}

impl ExactSizeIterator for ConfigArrayIter<'_, '_, '_> {
    fn len(&self) -> usize {
        self.values.len()
    }
}

impl FusedIterator for ConfigArrayIter<'_, '_, '_> {}

#[derive(Debug, Clone, Copy)]
enum ConfigLookupInner<'config, 'path> {
    Value {
        value: &'config ConfigValue,
        path: LookupPathSource<'path>,
    },
    Error {
        kind: LookupErrorKind<'path>,
        path: LookupPathSource<'path>,
    },
}

impl ConfigLookup<'_, '_> {
    fn is_empty_path(&self) -> bool {
        match self.inner {
            ConfigLookupInner::Value { path, .. } | ConfigLookupInner::Error { path, .. } => {
                matches!(path, LookupPathSource::FullPath(""))
            }
        }
    }
}

impl fmt::Display for ConfigLookup<'_, '_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.inner {
            ConfigLookupInner::Value { path, .. } | ConfigLookupInner::Error { path, .. } => {
                write!(f, "{}", path)
            }
        }
    }
}

impl<'config, 'path> ConfigLookup<'config, 'path> {
    fn from_full_path(path: &'path str, value: &'config ConfigValue) -> Self {
        ConfigLookup {
            inner: ConfigLookupInner::Value {
                value,
                path: LookupPathSource::FullPath(path),
            },
        }
    }

    fn error_path(path: &'path str, kind: LookupErrorKind<'path>) -> Self {
        ConfigLookup {
            inner: ConfigLookupInner::Error {
                kind,
                path: LookupPathSource::FullPath(path),
            },
        }
    }

    fn path_error(&self) -> ConfigPathError {
        match self.inner {
            ConfigLookupInner::Value { .. } => {
                unreachable!("value lookups do not produce path errors")
            }
            ConfigLookupInner::Error {
                kind: LookupErrorKind::InvalidPath,
                ..
            } => ConfigPathError::InvalidPath {
                path: self.to_string(),
            },
            ConfigLookupInner::Error {
                kind: LookupErrorKind::MissingKey,
                ..
            } => ConfigPathError::MissingKey {
                path: self.to_string(),
            },
            ConfigLookupInner::Error {
                kind: LookupErrorKind::MissingIndex,
                ..
            } => ConfigPathError::MissingIndex {
                path: self.to_string(),
            },
            ConfigLookupInner::Error {
                kind: LookupErrorKind::InvalidKeyAccess { key, value_type },
                ..
            } => ConfigPathError::InvalidKeyAccess {
                path: self.to_string(),
                key: key.to_string(),
                value_type,
            },
            ConfigLookupInner::Error {
                kind: LookupErrorKind::InvalidIndexAccess { index, value_type },
                ..
            } => ConfigPathError::InvalidIndexAccess {
                path: self.to_string(),
                index,
                value_type,
            },
        }
    }

    /// Select a child key from the current lookup value.
    pub fn get<'step>(&'step self, key: &'step str) -> ConfigLookup<'config, 'step> {
        let path = LookupPathSource::Child {
            parent: self,
            segment: PathSegment::Key(key),
            prepend_separator: !self.is_empty_path(),
        };
        match self.inner {
            ConfigLookupInner::Value { value, .. } => match &value.inner {
                ConfigValueInner::Table(values) => match values.get(key) {
                    Some(value) => ConfigLookup {
                        inner: ConfigLookupInner::Value { value, path },
                    },
                    None => ConfigLookup {
                        inner: ConfigLookupInner::Error {
                            kind: LookupErrorKind::MissingKey,
                            path,
                        },
                    },
                },
                _ => ConfigLookup {
                    inner: ConfigLookupInner::Error {
                        kind: LookupErrorKind::InvalidKeyAccess {
                            key,
                            value_type: value.value_type_name(),
                        },
                        path,
                    },
                },
            },
            ConfigLookupInner::Error { kind, .. } => ConfigLookup {
                inner: ConfigLookupInner::Error { kind, path },
            },
        }
    }

    /// Select a child index from the current lookup value.
    pub fn get_index<'step>(&'step self, index: usize) -> ConfigLookup<'config, 'step> {
        let path = LookupPathSource::Child {
            parent: self,
            segment: PathSegment::Index(index),
            prepend_separator: false,
        };
        match self.inner {
            ConfigLookupInner::Value { value, .. } => match &value.inner {
                ConfigValueInner::Array(values) => match values.get(index) {
                    Some(value) => ConfigLookup {
                        inner: ConfigLookupInner::Value { value, path },
                    },
                    None => ConfigLookup {
                        inner: ConfigLookupInner::Error {
                            kind: LookupErrorKind::MissingIndex,
                            path,
                        },
                    },
                },
                _ => ConfigLookup {
                    inner: ConfigLookupInner::Error {
                        kind: LookupErrorKind::InvalidIndexAccess {
                            index,
                            value_type: value.value_type_name(),
                        },
                        path,
                    },
                },
            },
            ConfigLookupInner::Error { kind, .. } => ConfigLookup {
                inner: ConfigLookupInner::Error { kind, path },
            },
        }
    }

    /// Return the underlying value or the captured path error.
    pub fn value(&self) -> Result<&'config ConfigValue, ConfigError> {
        match self.inner {
            ConfigLookupInner::Value { value, .. } => Ok(value),
            ConfigLookupInner::Error { .. } => Err(ConfigError::PathError(self.path_error())),
        }
    }

    /// Return the selected value as a string.
    pub fn as_string(&self) -> Result<String, ConfigError> {
        let value = self.value()?;
        value
            .as_string()
            .ok_or_else(|| self.expected::<String>(value))
    }

    /// Return the selected value as an integer.
    pub fn as_i64(&self) -> Result<i64, ConfigError> {
        let value = self.value()?;
        value.as_i64().ok_or_else(|| self.expected::<i64>(value))
    }

    /// Return the selected value as a float.
    pub fn as_f64(&self) -> Result<f64, ConfigError> {
        let value = self.value()?;
        value.as_f64().ok_or_else(|| self.expected::<f64>(value))
    }

    /// Return the selected value as a boolean.
    pub fn as_bool(&self) -> Result<bool, ConfigError> {
        let value = self.value()?;
        value.as_bool().ok_or_else(|| self.expected::<bool>(value))
    }

    /// Return the selected value as a TOML datetime.
    pub fn as_datetime(&self) -> Result<&'config toml::value::Datetime, ConfigError> {
        let value = self.value()?;
        value
            .as_datetime()
            .ok_or_else(|| self.expected::<toml::value::Datetime>(value))
    }

    /// Return the selected value as a byte size.
    pub fn as_bytes(&self) -> Result<u64, ConfigError> {
        let value = self.value()?;
        value.as_bytes().ok_or_else(|| self.expected::<u64>(value))
    }

    /// Return the selected value as a duration.
    pub fn as_duration(&self) -> Result<std::time::Duration, ConfigError> {
        let value = self.value()?;
        value
            .as_duration()
            .ok_or_else(|| self.expected::<std::time::Duration>(value))
    }

    /// Return the selected value as an array slice.
    ///
    /// ```
    /// use kompact::config::parse_config_str;
    ///
    /// let conf = parse_config_str("values = [1, 2, 3]").expect("config");
    /// let values = conf.select("values").as_array().expect("array");
    ///
    /// assert_eq!(3, values.len());
    /// ```
    pub fn as_array(&self) -> Result<&'config [ConfigValue], ConfigError> {
        let value = self.value()?;
        value
            .as_array()
            .ok_or_else(|| self.expected::<&[ConfigValue]>(value))
    }

    /// Iterate over the selected array value with path-aware element lookups.
    ///
    /// ```
    /// use kompact::config::parse_config_str;
    ///
    /// let conf = parse_config_str(r#"
    /// [[routes]]
    /// alias = "first"
    ///
    /// [[routes]]
    /// alias = "second"
    /// "#).expect("config");
    ///
    /// let routes = conf.select("routes");
    /// let mut aliases = Vec::new();
    /// for (_, route) in routes.array_entries().expect("array") {
    ///     aliases.push(route.get("alias").as_string().expect("alias"));
    /// }
    ///
    /// assert_eq!(vec!["first".to_string(), "second".to_string()], aliases);
    /// ```
    pub fn array_entries<'lookup>(
        &'lookup self,
    ) -> Result<ConfigArrayIter<'config, 'lookup, 'path>, ConfigError> {
        let values = self.as_array()?;
        Ok(ConfigArrayIter {
            parent: self,
            values: values.iter().enumerate(),
        })
    }

    fn expected<T>(&self, value: &ConfigValue) -> ConfigError {
        match self.inner {
            ConfigLookupInner::Value { .. } => {
                ConfigError::expected_at::<T>(&self.to_string(), value)
            }
            ConfigLookupInner::Error { .. } => ConfigError::PathError(self.path_error()),
        }
    }
}

/// Errors raised while traversing a parsed configuration value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigPathError {
    /// The requested path string is malformed.
    InvalidPath {
        /// The malformed config path.
        path: String,
    },
    /// The requested key was missing.
    MissingKey {
        /// The full missing config path.
        path: String,
    },
    /// The requested array element was missing.
    MissingIndex {
        /// The full missing config path.
        path: String,
    },
    /// A key access was attempted on a non-table value.
    InvalidKeyAccess {
        /// The full config path that was being traversed.
        path: String,
        /// The key fragment whose access failed.
        key: String,
        /// The type of the parent value that rejected the key access.
        value_type: &'static str,
    },
    /// An index access was attempted on a non-array value.
    InvalidIndexAccess {
        /// The full config path that was being traversed.
        path: String,
        /// The index fragment whose access failed.
        index: usize,
        /// The type of the parent value that rejected the index access.
        value_type: &'static str,
    },
}

impl ConfigPathError {
    /// Returns `true` if the error denotes a missing path element.
    pub fn is_missing(&self) -> bool {
        matches!(
            self,
            ConfigPathError::MissingKey { .. } | ConfigPathError::MissingIndex { .. }
        )
    }
}

impl fmt::Display for ConfigPathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigPathError::InvalidPath { path } => write!(f, "invalid config path `{}`", path),
            ConfigPathError::MissingKey { path } => write!(f, "missing config key at `{}`", path),
            ConfigPathError::MissingIndex { path } => {
                write!(f, "missing config index at `{}`", path)
            }
            ConfigPathError::InvalidKeyAccess {
                path,
                key,
                value_type,
            } => write!(
                f,
                "cannot access key `{}` at `{}` because the parent value is a {}",
                key, path, value_type
            ),
            ConfigPathError::InvalidIndexAccess {
                path,
                index,
                value_type,
            } => write!(
                f,
                "cannot access index [{}] at `{}` because the parent value is a {}",
                index, path, value_type
            ),
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
}

impl fmt::Display for ConfigLoadingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigLoadingError::Io(err) => write!(f, "failed to read config file: {}", err),
            ConfigLoadingError::Parse(err) => write!(f, "failed to parse TOML config: {}", err),
        }
    }
}

impl Error for ConfigLoadingError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ConfigLoadingError::Io(err) => Some(err),
            ConfigLoadingError::Parse(err) => Some(err),
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

/// Parse a TOML string into a configuration document.
pub fn parse_config_str(config_string: &str) -> Result<Config, ConfigLoadingError> {
    let config: toml::Table = toml::from_str(config_string)?;
    Ok(Config::from_toml_table(config))
}

/// Parse a TOML file into a configuration document.
pub fn parse_config_file<P>(path: P) -> Result<Config, ConfigLoadingError>
where
    P: AsRef<Path>,
{
    let config_string = std::fs::read_to_string(path)?;
    parse_config_str(config_string.as_ref())
}

fn merge_tables(current: &mut ConfigTable, next: ConfigTable) {
    for (key, value) in next {
        if let Some(existing) = current.get_mut(&key) {
            existing.merge(value);
        } else {
            current.insert(key, value);
        }
    }
}

fn insert_segments(current: &mut ConfigTable, segments: &[&str], value: ConfigValue) {
    if segments.is_empty() {
        return;
    }

    if segments.len() == 1 {
        if let Some(existing) = current.get_mut(segments[0]) {
            existing.merge(value);
        } else {
            current.insert(segments[0].to_string(), value);
        }
        return;
    }

    let child = current
        .entry(segments[0].to_string())
        .or_insert_with(ConfigValue::table);

    if !matches!(child.inner, ConfigValueInner::Table(_)) {
        *child = ConfigValue::table();
    }

    let values = match &mut child.inner {
        ConfigValueInner::Table(values) => values,
        _ => unreachable!("table values should stay tables"),
    };
    insert_segments(values, &segments[1..], value);
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
    pub fn select<'config>(&self, conf: &'config Config) -> ConfigLookup<'config, 'static> {
        conf.select(self.key)
    }

    /// Performs the validation of the given value.
    ///
    /// If no validator is specified, the value is simply passed through.
    pub fn validate(&self, value: T::Value) -> Result<T::Value, ConfigError> {
        if let Some(validator) = self.validator {
            match validator(&value) {
                Ok(_) => Ok(value),
                Err(err_msg) => Err(ConfigError::InvalidValue {
                    path: Some(self.key.to_string()),
                    description: err_msg,
                }),
            }
        } else {
            Ok(value)
        }
    }

    /// Read the value for this key from the given config.
    pub fn read(&self, conf: &Config) -> Result<T::Value, ConfigError> {
        let value = self.select(conf).value()?;
        let value = T::from_conf(value).map_err(|error| error.with_path(self.key))?;
        self.validate(value)
    }

    /// Read the value for this key from the given config or return the default value if the path is not present.
    pub fn read_or_default(&self, conf: &Config) -> Result<T::Value, ConfigError> {
        match self.read(conf) {
            Ok(value) => Ok(value),
            Err(ConfigError::PathError(error)) if error.is_missing() => match self.default() {
                Some(default) => Ok(default),
                None => Err(ConfigError::PathError(error)),
            },
            Err(error) => Err(error),
        }
    }
}

/// A value extractor for config values.
pub trait ConfigValueType {
    /// The type of the value extracted by this type.
    type Value;

    /// Extract the value from a config instance.
    fn from_conf(conf: &ConfigValue) -> Result<Self::Value, ConfigError>;

    /// Convert a runtime value into a config value.
    fn into_config_value(value: Self::Value) -> ConfigValue;

    /// Produce a TOML fragment for the value.
    fn config_string(value: Self::Value) -> String {
        Self::into_config_value(value).to_toml_fragment()
    }
}

/// Errors that occur during config lookup.
#[derive(Debug, Clone, PartialEq)]
pub enum ConfigError {
    /// Type conversion failed.
    ConversionError {
        /// The config path where the conversion was attempted, if known.
        path: Option<String>,
        /// A human-readable description of the failure.
        description: String,
    },
    /// Path traversal failed.
    PathError(ConfigPathError),
    /// Value validation failed.
    InvalidValue {
        /// The config path that failed validation, if known.
        path: Option<String>,
        /// A human-readable description of the failure.
        description: String,
    },
}

impl ConfigError {
    fn expected<T>(conf: &ConfigValue) -> Self {
        ConfigError::ConversionError {
            path: None,
            description: format!(
                "Expected {} config value, but got {:?}",
                std::any::type_name::<T>(),
                conf
            ),
        }
    }

    fn expected_at<T>(path: &str, conf: &ConfigValue) -> Self {
        ConfigError::ConversionError {
            path: Some(path.to_string()),
            description: format!(
                "Expected {} config value, but got {:?}",
                std::any::type_name::<T>(),
                conf
            ),
        }
    }

    fn with_path(self, path: &str) -> Self {
        match self {
            ConfigError::ConversionError {
                path: Some(existing),
                description,
            } => ConfigError::ConversionError {
                path: Some(existing),
                description,
            },
            ConfigError::ConversionError {
                path: None,
                description,
            } => ConfigError::ConversionError {
                path: Some(path.to_string()),
                description,
            },
            ConfigError::InvalidValue {
                path: Some(existing),
                description,
            } => ConfigError::InvalidValue {
                path: Some(existing),
                description,
            },
            ConfigError::InvalidValue {
                path: None,
                description,
            } => ConfigError::InvalidValue {
                path: Some(path.to_string()),
                description,
            },
            other => other,
        }
    }
}

impl From<ConfigPathError> for ConfigError {
    fn from(error: ConfigPathError) -> Self {
        ConfigError::PathError(error)
    }
}

impl From<std::num::TryFromIntError> for ConfigError {
    fn from(error: std::num::TryFromIntError) -> Self {
        ConfigError::ConversionError {
            path: None,
            description: error.to_string(),
        }
    }
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::ConversionError { path, description } => match path {
                Some(path) => write!(
                    f,
                    "Error during type conversion at `{}`: {}",
                    path, description
                ),
                None => write!(f, "Error during type conversion: {}", description),
            },
            ConfigError::PathError(error) => write!(f, "Error during path traversal: {}", error),
            ConfigError::InvalidValue { path, description } => match path {
                Some(path) => write!(
                    f,
                    "Error during value validation at `{}`: {}",
                    path, description
                ),
                None => write!(f, "Error during value validation: {}", description),
            },
        }
    }
}

impl Error for ConfigError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            ConfigError::PathError(error) => Some(error),
            ConfigError::ConversionError { .. } | ConfigError::InvalidValue { .. } => None,
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

    const ARRAY_CONFIG: &str = r#"
    values = [1, 2, 3]
    empty = []

    [[routes]]
    alias = "first"

    [[routes]]
    alias = "second"
    "#;

    #[test]
    fn simple_config_key() {
        assert_eq!("kompact.my-test-key", SIMPLE_KEY.key);
        assert_eq!(vec!["kompact", "my-test-key"], SIMPLE_KEY.path_segments());

        let conf = parse_config_str(EXAMPLE_CONFIG).expect("config");
        let value = SIMPLE_KEY.read(&conf).expect("String");
        assert_eq!("testme", value);

        let nested = KEY_FROM_MACRO_NO_DEFAULT.read(&conf).expect("String");
        assert_eq!("test me inside", nested);
    }

    #[test]
    fn default_config_key() {
        let conf = parse_config_str(EXAMPLE_CONFIG).expect("config");
        let value = KEY_WITH_DEFAULT.read_or_default(&conf).expect("String");
        assert_eq!("default string", value);

        let nested = KEY_FROM_MACRO.read_or_default(&conf).expect("String");
        assert_eq!("default value", nested);
    }

    #[test]
    fn validated_config_key() {
        {
            let conf = parse_config_str(EXAMPLE_CONFIG).expect("config");

            let value = conf.read(&KEY_FROM_MACRO_VALIDATE).unwrap();
            assert_eq!(50, value);
        }
        {
            let conf = parse_config_str(BAD_CONFIG).expect("config");

            let res = conf.read(&KEY_FROM_MACRO_VALIDATE);
            assert!(res.is_err());
        }
    }

    #[test]
    fn simple_key_bad_config() {
        let conf = parse_config_str(BAD_CONFIG).expect("config");
        let res = SIMPLE_KEY.read(&conf);
        assert_eq!(
            Err(ConfigError::PathError(ConfigPathError::MissingKey {
                path: "kompact.my-test-key".to_string()
            })),
            res
        );
    }

    #[test]
    fn lookup_reports_full_missing_path() {
        let conf = parse_config_str(EXAMPLE_CONFIG).expect("config");
        let res = conf.select("kompact.test-group.other-key").as_string();
        assert_eq!(
            Err(ConfigError::PathError(ConfigPathError::MissingKey {
                path: "kompact.test-group.other-key".to_string()
            })),
            res
        );
    }

    #[test]
    fn lookup_reports_invalid_parent_type() {
        let conf = parse_config_str(EXAMPLE_CONFIG).expect("config");
        let res = conf.select("kompact.my-test-key.inner").as_string();
        assert_eq!(
            Err(ConfigError::PathError(ConfigPathError::InvalidKeyAccess {
                path: "kompact.my-test-key.inner".to_string(),
                key: "inner".to_string(),
                value_type: "string"
            })),
            res
        );
    }

    #[test]
    fn lookup_propagates_additional_key_fragments_after_failure() {
        let conf = parse_config_str(EXAMPLE_CONFIG).expect("config");
        let res = conf
            .get("kompact")
            .get("my-test-key")
            .get("inner")
            .get("leaf")
            .as_string();
        assert_eq!(
            Err(ConfigError::PathError(ConfigPathError::InvalidKeyAccess {
                path: "kompact.my-test-key.inner.leaf".to_string(),
                key: "inner".to_string(),
                value_type: "string"
            })),
            res
        );
    }

    #[test]
    fn config_value_returns_array_slice() {
        let conf = parse_config_str(ARRAY_CONFIG).expect("config");
        let values = conf["values"].as_array().expect("array");

        assert_eq!(3, values.len());
        assert_eq!(Some(1), values[0].as_i64());
        assert_eq!(Some(2), values[1].as_i64());
        assert_eq!(Some(3), values[2].as_i64());
        assert_eq!(None, conf["routes"][0]["alias"].as_array());
    }

    #[test]
    fn lookup_returns_array_slice() {
        let conf = parse_config_str(ARRAY_CONFIG).expect("config");
        let values = conf.select("values").as_array().expect("array");

        assert_eq!(3, values.len());
        assert_eq!(Some(2), values[1].as_i64());
    }

    #[test]
    fn lookup_array_slice_reports_non_array_conversion_error() {
        let conf = parse_config_str(EXAMPLE_CONFIG).expect("config");
        let res = conf.select("kompact.my-test-key").as_array();

        match res {
            Err(ConfigError::ConversionError {
                path: Some(path), ..
            }) => assert_eq!("kompact.my-test-key", path),
            other => panic!("expected conversion error with path, got {:?}", other),
        }
    }

    #[test]
    fn lookup_array_entries_iterates_available_indices() {
        let conf = parse_config_str(ARRAY_CONFIG).expect("config");
        let routes = conf.select("routes");
        let entries: Vec<_> = routes
            .array_entries()
            .expect("routes")
            .map(|(index, route)| (index, route.get("alias").as_string().expect("route alias")))
            .collect();

        assert_eq!(
            vec![(0, "first".to_string()), (1, "second".to_string())],
            entries
        );
    }

    #[test]
    fn lookup_array_entries_supports_empty_arrays() {
        let conf = parse_config_str(ARRAY_CONFIG).expect("config");
        let empty = conf.select("empty");
        let entries = empty.array_entries().expect("empty array");

        assert_eq!(0, entries.len());
        assert_eq!((0, Some(0)), entries.size_hint());
    }

    #[test]
    fn lookup_array_entries_reports_child_paths() {
        let conf = parse_config_str(ARRAY_CONFIG).expect("config");
        let routes = conf.select("routes");
        let mut entries = routes.array_entries().expect("routes");
        let (_, first_route) = entries.next().expect("first route");
        let res = first_route.get("missing").as_string();

        assert_eq!(
            Err(ConfigError::PathError(ConfigPathError::MissingKey {
                path: "routes[0].missing".to_string()
            })),
            res
        );
    }

    #[test]
    fn lookup_array_entries_reports_missing_paths() {
        let conf = parse_config_str(ARRAY_CONFIG).expect("config");
        let missing_lookup = conf.select("missing");
        let missing = missing_lookup.array_entries();

        match missing {
            Err(ConfigError::PathError(ConfigPathError::MissingKey { path })) => {
                assert_eq!("missing", path);
            }
            other => panic!("expected missing path error, got {:?}", other),
        }
    }

    pub(super) fn str_conf(config_string: &str) -> Config {
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
