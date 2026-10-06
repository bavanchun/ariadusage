//! Core domain model, provider descriptors, and pure business logic for AriadUsage.

pub mod config;
pub mod error;
pub mod model;
pub mod pace;
pub mod projection;
pub mod providers;
pub mod settings_value;

pub use config::*;
pub use error::ModelError;
pub use model::*;
pub use pace::*;
pub use projection::*;
pub use providers::{
    ProviderDescriptor, SourceMode, find_by_id, find_by_id_str, first_party_order,
};
pub use settings_value::SettingsValue;
