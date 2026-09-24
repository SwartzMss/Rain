pub mod metadata;
mod model;
mod service;
mod validation;

pub use model::{
    AdaptiveMode, AdaptiveModes, ApplyMode, SaveResult, SettingKey, SettingValue, SettingsSnapshot,
    SettingsValues, ValidationError,
};
pub use service::SettingsService;
