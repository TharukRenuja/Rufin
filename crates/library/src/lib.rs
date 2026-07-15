//! Defines Rufin's library items and stores them for each music source.
//!
//! Source clients provide data, `library-sync` applies changes, and the UI
//! decides how the items are displayed.

macro_rules! opaque_id {
    ($name:ident, $prefix:literal) => {
        #[derive(
            Clone,
            Debug,
            serde::Deserialize,
            Eq,
            Hash,
            Ord,
            PartialEq,
            PartialOrd,
            serde::Serialize,
        )]
        pub struct $name(String);

        impl $name {
            pub fn new(value: impl Into<String>) -> Self {
                let value = value.into();
                assert!(
                    !value.is_empty(),
                    concat!(stringify!($name), " cannot be empty")
                );
                Self(value)
            }

            pub fn fake(number: impl std::fmt::Display) -> Self {
                Self::new(format!("{}{}", $prefix, number))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::new(value)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str(&self.0)
            }
        }
    };
}

mod active_query;
pub mod collections;
mod events;
pub mod home;
pub mod items;
pub mod local_manifest;
pub mod play_context;
pub mod queries;
pub mod smart_playlists;
pub mod source_mapping;
mod store;

pub use active_query::*;
pub use collections::*;
pub use events::*;
pub use home::*;
pub use items::*;
pub use local_manifest::*;
pub use queries::*;
pub use smart_playlists::*;
pub use source_mapping::*;
pub use store::*;

pub(crate) const fn msgid(message: &'static str) -> &'static str {
    message
}
