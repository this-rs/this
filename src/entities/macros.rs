//! Macros for reducing boilerplate when defining entities
//!
//! These macros generate the repetitive trait implementations needed
//! for each entity type.

/// Implement the Data trait for an entity
///
/// # Example
///
/// ```rust,ignore
/// use this::prelude::*;
///
/// #[derive(Debug, Clone, Serialize, Deserialize)]
/// struct User {
///     id: Uuid,
///     tenant_id: Uuid,
///     name: String,
///     email: String,
/// }
///
/// impl_data_entity!(User, "user", ["name", "email"]);
/// ```
#[macro_export]
macro_rules! impl_data_entity {
    ($type:ty, $singular:expr, [$($field:expr),*]) => {
        impl $crate::core::entity::Entity for $type {
            type Service = (); // To be overridden by user

            fn resource_name() -> &'static str {
                // Fix: Use Box::leak to safely create 'static str
                use std::sync::OnceLock;
                static PLURAL: OnceLock<&'static str> = OnceLock::new();
                PLURAL.get_or_init(|| {
                    Box::leak(
                        $crate::core::pluralize::Pluralizer::pluralize($singular)
                            .into_boxed_str()
                    )
                })
            }

            fn resource_name_singular() -> &'static str {
                $singular
            }

            fn service_from_host(
                _host: &std::sync::Arc<dyn std::any::Any + Send + Sync>
            ) -> anyhow::Result<std::sync::Arc<Self::Service>> {
                unimplemented!("service_from_host must be implemented by user")
            }
        }

        impl $crate::core::entity::Data for $type {
            fn id(&self) -> uuid::Uuid {
                self.id
            }

            fn tenant_id(&self) -> uuid::Uuid {
                self.tenant_id
            }

            fn indexed_fields() -> &'static [&'static str] {
                &[$($field),*]
            }

            fn field_value(&self, field: &str) -> Option<$crate::core::field::FieldValue> {
                match field {
                    $(
                        $field => Some($crate::core::field::FieldValue::String(
                            self.$field.to_string()
                        )),
                    )*
                    _ => None,
                }
            }
        }
    };
}

// Note: impl_crud_handlers! would be a procedural macro for generating
// HTTP handlers. This is a placeholder for the concept.

#[cfg(test)]
mod tests {
    use crate::prelude::*;

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct TestUser {
        id: Uuid,
        tenant_id: Uuid,
        name: String,
        email: String,
    }

    // This won't work in doc tests, but shows the usage
    // impl_data_entity!(TestUser, "test_user", ["name", "email"]);
}
