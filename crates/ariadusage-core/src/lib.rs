//! Core domain model, provider descriptors, and pure business logic for AriadUsage.

pub mod error;
pub mod model;
pub mod providers;
pub mod settings_value;

pub use error::ModelError;
pub use model::*;
pub use providers::{
    ProviderDescriptor, SourceMode, find_by_id, find_by_id_str, first_party_order,
};
pub use settings_value::SettingsValue;
