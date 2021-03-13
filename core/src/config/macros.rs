use super::*;

// /// Macro to create config groups
// #[macro_export]
// macro_rules! kompact_config_group {
//     ($name:ident, key = $key:literal, parent = $parent:path, doc = $doc:literal, version = $version:literal) => {
//         impl_generate_group!($name, $key, Some(&$parent), stringify!($parent), $doc, $version);
//     };
//     ($name:ident, key = $key:literal, doc = $doc:literal, version = $version:literal) => {
//         impl_generate_group!($name, $key, $doc, $version);
//     };
//     ($name:ident, key = $key:literal, parent = $parent:path, version = $version:literal) => {
//         impl_generate_group!($name, $key, Some(&$parent), stringify!($parent), "A Kompact configuration group.", $version);
//     };
//     ($name:ident, key = $key:literal, version = $version:literal) => {
//         impl_generate_group!($name, $key, "A Kompact top-level configuration group.", $version);
//     };
// }

// #[macro_export]
// #[doc(hidden)]
// macro_rules! impl_generate_group {
// 	($name:ident, $key:literal, $parent_expr:expr, $parent_string:literal, $doc:literal, $version:literal) => {
//         #[doc = "group key: `"]
//         #[doc = $key]
//         #[doc = "` [parent]("]
//         #[doc = $parent_string]
//         #[doc = ")\n# Description \n"]
//         #[doc = $doc]
//         #[doc = "\n# Since\n Kompact version "]
//         #[doc = $version]
//         pub const $name: $crate::config::ConfigGroup = $crate::config::ConfigGroup {
//             key: $key,
//             parent: $parent_expr,
//             doc: $doc,
//             version: $version,
//         };
//     };
//     ($name:ident, $key:literal, $doc:literal, $version:literal) => {
//         #[doc = "group key: `"]
//         #[doc = $key]
//         #[doc = "`\n# Description \n"]
//         #[doc = $doc]
//         #[doc = "\n# Since\n Kompact version "]
//         #[doc = $version]
//         pub const $name: $crate::config::ConfigGroup = $crate::config::ConfigGroup {
//             key: $key,
//             parent: $parent_expr,
//             doc: $doc,
//             version: $version,
//         };
//     };
// }


// #[macro_export]
// #[doc(hidden)]
// macro_rules! last_in_path {
// 	($id:ident) => {
// 		stringify!($id)
// 	};
// 	($prefix:ident :: $($rest:tt)*) => {
// 		last_in_path!($($rest)*)
// 	};
// 	($p:path) => {
// 		compile_error!(concat!("Can't take apart path: ", stringify!($p)));
// 	}
// }

// #[cfg(test)]
// mod tests {
// 	use super::*;
	
// 	macro_rules! path_to_segments {
// 		($id:ident) => {
// 			[stringify!($id)]
// 		};
// 		($(tokens:tt)*) => {
			
// 		};
// 	}

// 	#[test]
// 	fn destructure_a_path() {
// 		trace_macros!(true);
// 		let res = path_to_segments!(kompact.my-test-group.my-key);
// 		trace_macros!(false);
// 		assert_eq!("KOMPACT", res);
// 	}
// }
