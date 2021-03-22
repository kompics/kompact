/// Macro to create config entries.
///
/// This macro also generated rustdoc that is consistent with the key and the doc field.
#[macro_export]
macro_rules! kompact_config {
	($name:ident,
    key = $key:literal,
    type = $value_type:ty,
    default = $default:expr,
    validate = |$value_id:ident| $validate:expr,
    doc = $doc:literal,
    version = $version:literal) => {
        #[doc = "(`"]
        #[doc = $key]
        #[doc = "`) "]
        #[doc = $doc]
        #[doc = "\n# Since\n Kompact version "]
        #[doc = $version]
        pub const $name: $crate::config::ConfigEntry<$value_type> = {
            fn default_value() -> <$value_type as $crate::config::ConfigValueType>::Value {
                $default
            }
            fn validate_value($value_id: &<$value_type as $crate::config::ConfigValueType>::Value) -> Result<(), String> {
            	if $validate {
            		Ok(())
            	} else {
            		Err(format!("Value {} did satisfy {}", $value_id, stringify!($validate)))
            	}
            }
            $crate::config::ConfigEntry {
                key: $key,
                doc: $doc,
                version: $version,
                value_type: ::std::marker::PhantomData,
                default: Some(default_value),
                validator: Some(validate_value),
            }
        };
    };
    ($name:ident,
    key = $key:literal,
    type = $value_type:ty,
    default = $default:expr,
    doc = $doc:literal,
    version = $version:literal) => {
        #[doc = "(`"]
        #[doc = $key]
        #[doc = "`) "]
        #[doc = $doc]
        #[doc = "\n# Since\n Kompact version "]
        #[doc = $version]
        pub const $name: $crate::config::ConfigEntry<$value_type> = {
            fn default_value() -> <$value_type as $crate::config::ConfigValueType>::Value {
                $default
            }
            $crate::config::ConfigEntry {
                key: $key,
                doc: $doc,
                version: $version,
                value_type: ::std::marker::PhantomData,
                default: Some(default_value),
                validator: None,
            }
        };
    };
    ($name:ident,
    key = $key:literal,
    type = $value_type:ty,
    doc = $doc:literal,
    version = $version:literal) => {
        #[doc = "(`"]
        #[doc = $key]
        #[doc = "`) "]
        #[doc = $doc]
        #[doc = "\n# Default\n This config entry has no default value."]
        #[doc = "\n# Since\n Kompact version "]
        #[doc = $version]
        pub const $name: $crate::config::ConfigEntry<$value_type> = $crate::config::ConfigEntry {
            key: $key,
            doc: $doc,
            version: $version,
            value_type: ::std::marker::PhantomData,
            default: None,
            validator: None,
        };
    };
    ($name:ident,
    key = $key:literal,
    doc = $doc:literal,
    version = $version:literal) => {
        kompact_config!($name, key = $key, type = $crate::config::StringValue, doc = $doc, version = $version);
    };
}

macro_rules! config_assert {
    ($cond:expr, $val:ident) => {
        #[allow(clippy::neg_cmp_op_on_partial_ord)]
        if !($cond) {
            let condition = stringify!($cond).replace(stringify!($val), "value");
            let error_msg = format!("value={} did not satisfy condition `{}`", $val, condition);
            return Err($crate::config::ConfigError::ConversionError(error_msg));
        }
    };
}
