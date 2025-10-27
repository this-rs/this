//! Validation and filtering system
//!
//! This module provides a declarative approach to validating and filtering entity data
//! before it reaches the handlers. It integrates seamlessly with the entity macro system.

pub mod config;
pub mod extractor;
pub mod filters;
pub mod validators;

pub use config::EntityValidationConfig;
pub use extractor::Validated;
