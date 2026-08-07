//! Everything about a request that is independent of how it is displayed or
//! sent: the on-disk model, the collection tree, settings, and variables.

pub mod collection;
pub mod config;
pub mod env;
pub mod files;
pub mod header_names;
pub mod locations;
pub mod model;
pub mod template;
pub mod urls;
pub mod variables;
pub mod yaml;

pub use collection::Collection;
pub use config::Settings;
pub use model::{
    Auth, AuthKind, BodyContent, HttpMethod, KeyValue, Options, PathParam, RequestModel,
    ScriptHook, ScriptRef, Scripts,
};
pub use variables::Variables;
