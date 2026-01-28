//! Macros for reducing boilerplate when defining entities
//!
//! These macros generate the repetitive trait implementations needed
//! for each entity type following the Entity/Data/Link architecture.

/// Helper macro to enable multi-tenancy for an entity
///
/// This macro adds an override for the `Entity::tenant_id()` method
/// to return the actual tenant_id field value.
///
/// # Example
/// ```rust,ignore
/// impl_data_entity!(User, "user", ["name"], {
///     tenant_id: Uuid,
///     email: String,
/// });
///
/// // Enable multi-tenancy
/// impl_entity_multi_tenant!(User);
/// ```
#[macro_export]
macro_rules! impl_entity_multi_tenant {
    ($type:ident) => {
        // Cannot override trait methods in separate impl blocks in stable Rust
        // This is a marker for documentation purposes
        // Users should manually implement tenant_id access via a helper method
        impl $type {
            /// Get the tenant ID for multi-tenant isolation
            #[allow(dead_code)]
            pub fn get_tenant_id(&self) -> ::uuid::Uuid {
                self.tenant_id
            }
        }
    };
}

/// Macro to inject Entity base fields into a struct
///
/// Injects: id, entity_type, created_at, updated_at, deleted_at, status
#[macro_export]
macro_rules! entity_fields {
    () => {
        /// Unique identifier for this entity
        pub id: ::uuid::Uuid,

        /// Type of the entity (e.g., "user", "product")
        #[serde(rename = "type")]
        pub entity_type: String,

        /// When this entity was created
        pub created_at: ::chrono::DateTime<::chrono::Utc>,

        /// When this entity was last updated
        pub updated_at: ::chrono::DateTime<::chrono::Utc>,

        /// When this entity was soft-deleted (if applicable)
        pub deleted_at: Option<::chrono::DateTime<::chrono::Utc>>,

        /// Current status of the entity
        pub status: String,
    };
}

/// Macro to inject Data fields into a struct (Entity fields + name)
#[macro_export]
macro_rules! data_fields {
    () => {
        /// Unique identifier for this entity
        pub id: ::uuid::Uuid,

        /// Type of the entity (e.g., "user", "product")
        #[serde(rename = "type")]
        pub entity_type: String,

        /// When this entity was created
        pub created_at: ::chrono::DateTime<::chrono::Utc>,

        /// When this entity was last updated
        pub updated_at: ::chrono::DateTime<::chrono::Utc>,

        /// When this entity was soft-deleted (if applicable)
        pub deleted_at: Option<::chrono::DateTime<::chrono::Utc>>,

        /// Current status of the entity
        pub status: String,

        /// Name of this data entity
        pub name: String,
    };
}

/// Macro to inject Link fields into a struct (Entity fields + source_id + target_id + link_type)
#[macro_export]
macro_rules! link_fields {
    () => {
        /// Unique identifier for this entity
        pub id: ::uuid::Uuid,

        /// Type of the entity (e.g., "user", "product")
        #[serde(rename = "type")]
        pub entity_type: String,

        /// When this entity was created
        pub created_at: ::chrono::DateTime<::chrono::Utc>,

        /// When this entity was last updated
        pub updated_at: ::chrono::DateTime<::chrono::Utc>,

        /// When this entity was soft-deleted (if applicable)
        pub deleted_at: Option<::chrono::DateTime<::chrono::Utc>>,

        /// Current status of the entity
        pub status: String,

        /// Type of relationship
        pub link_type: String,

        /// ID of the source entity
        pub source_id: ::uuid::Uuid,

        /// ID of the target entity
        pub target_id: ::uuid::Uuid,
    };
}

/// Complete macro to create a Data entity with automatic trait implementations
///
/// # Example
///
/// ```rust,ignore
/// use this::prelude::*;
///
/// impl_data_entity!(
///     User,
///     "user",
///     ["name", "email"],
///     {
///         email: String,
///         password_hash: String,
///         roles: Vec<String>,
///     }
/// );
///
/// // Usage
/// let user = User::new(
///     "John Doe".to_string(),
///     "active".to_string(),
///     "john@example.com".to_string(),
///     "$argon2$...".to_string(),
///     vec!["admin".to_string()],
/// );
/// ```
#[macro_export]
macro_rules! impl_data_entity {
    (
        $type:ident,
        $type_name:expr,
        [ $( $indexed_field:expr ),* $(,)? ],
        {
            $( $specific_field:ident : $specific_type:ty ),* $(,)?
        }
    ) => {
        #[derive(Debug, Clone, ::serde::Serialize, ::serde::Deserialize)]
        pub struct $type {
            /// Unique identifier for this entity
            pub id: ::uuid::Uuid,

            /// Type of the entity
            #[serde(rename = "type")]
            pub entity_type: String,

            /// When this entity was created
            pub created_at: ::chrono::DateTime<::chrono::Utc>,

            /// When this entity was last updated
            pub updated_at: ::chrono::DateTime<::chrono::Utc>,

            /// When this entity was soft-deleted (if applicable)
            pub deleted_at: Option<::chrono::DateTime<::chrono::Utc>>,

            /// Current status of the entity
            pub status: String,

            /// Name of this data entity
            pub name: String,
            $( pub $specific_field : $specific_type ),*
        }

        // Implement Entity trait
        impl $crate::core::entity::Entity for $type {
            fn resource_name() -> &'static str {
                use std::sync::OnceLock;
                static PLURAL: OnceLock<String> = OnceLock::new();
                PLURAL.get_or_init(|| {
                    $crate::core::pluralize::Pluralizer::pluralize($type_name)
                }).as_str()
            }

            fn resource_name_singular() -> &'static str {
                $type_name
            }

            fn id(&self) -> ::uuid::Uuid {
                self.id
            }

            fn entity_type(&self) -> &str {
                &self.entity_type
            }

            fn created_at(&self) -> ::chrono::DateTime<::chrono::Utc> {
                self.created_at
            }

            fn updated_at(&self) -> ::chrono::DateTime<::chrono::Utc> {
                self.updated_at
            }

            fn deleted_at(&self) -> Option<::chrono::DateTime<::chrono::Utc>> {
                self.deleted_at
            }

            fn status(&self) -> &str {
                &self.status
            }
        }

        // Implement Data trait
        impl $crate::core::entity::Data for $type {
            fn name(&self) -> &str {
                &self.name
            }

            fn indexed_fields() -> &'static [&'static str] {
                &[ $( $indexed_field ),* ]
            }

            fn field_value(&self, field: &str) -> Option<$crate::core::field::FieldValue> {
                match field {
                    "name" => Some($crate::core::field::FieldValue::String(self.name.clone())),
                    "status" => Some($crate::core::field::FieldValue::String(self.status.clone())),
                    _ => None,
                }
            }
        }

        // Utility methods
        impl $type {
            /// Create a new instance of this entity
            pub fn new(
                name: String,
                status: String,
                $( $specific_field: $specific_type ),*
            ) -> Self {
                Self {
                    id: ::uuid::Uuid::new_v4(),
                    entity_type: $type_name.to_string(),
                    created_at: ::chrono::Utc::now(),
                    updated_at: ::chrono::Utc::now(),
                    deleted_at: None,
                    status,
                    name,
                    $( $specific_field ),*
                }
            }

            /// Soft delete this entity (sets deleted_at timestamp)
            pub fn soft_delete(&mut self) {
                self.deleted_at = Some(::chrono::Utc::now());
                self.updated_at = ::chrono::Utc::now();
            }

            /// Restore a soft-deleted entity (clears deleted_at timestamp)
            pub fn restore(&mut self) {
                self.deleted_at = None;
                self.updated_at = ::chrono::Utc::now();
            }

            /// Update the updated_at timestamp to now
            pub fn touch(&mut self) {
                self.updated_at = ::chrono::Utc::now();
            }

            /// Change the entity status
            pub fn set_status(&mut self, status: String) {
                self.status = status;
                self.touch();
            }
        }
    };
}

/// Complete macro to create a Link entity with automatic trait implementations
///
/// # Example
///
/// ```rust,ignore
/// use this::prelude::*;
///
/// impl_link_entity!(
///     UserCompanyLink,
///     "user_company_link",
///     {
///         role: String,
///         start_date: DateTime<Utc>,
///     }
/// );
///
/// // Usage
/// let link = UserCompanyLink::new(
///     "employment".to_string(),
///     user_id,
///     company_id,
///     "active".to_string(),
///     "Senior Developer".to_string(),
///     Utc::now(),
/// );
/// ```
#[macro_export]
macro_rules! impl_link_entity {
    (
        $type:ident,
        $type_name:expr,
        {
            $( $specific_field:ident : $specific_type:ty ),* $(,)?
        }
    ) => {
        #[derive(Debug, Clone, ::serde::Serialize, ::serde::Deserialize)]
        pub struct $type {
            /// Unique identifier for this entity
            pub id: ::uuid::Uuid,

            /// Type of the entity
            #[serde(rename = "type")]
            pub entity_type: String,

            /// When this entity was created
            pub created_at: ::chrono::DateTime<::chrono::Utc>,

            /// When this entity was last updated
            pub updated_at: ::chrono::DateTime<::chrono::Utc>,

            /// When this entity was soft-deleted (if applicable)
            pub deleted_at: Option<::chrono::DateTime<::chrono::Utc>>,

            /// Current status of the entity
            pub status: String,

            /// Type of relationship
            pub link_type: String,

            /// ID of the source entity
            pub source_id: ::uuid::Uuid,

            /// ID of the target entity
            pub target_id: ::uuid::Uuid,
            $( pub $specific_field : $specific_type ),*
        }

        // Implement Entity trait
        impl $crate::core::entity::Entity for $type {
            fn resource_name() -> &'static str {
                use std::sync::OnceLock;
                static PLURAL: OnceLock<String> = OnceLock::new();
                PLURAL.get_or_init(|| {
                    $crate::core::pluralize::Pluralizer::pluralize($type_name)
                }).as_str()
            }

            fn resource_name_singular() -> &'static str {
                $type_name
            }

            fn id(&self) -> ::uuid::Uuid {
                self.id
            }

            fn entity_type(&self) -> &str {
                &self.entity_type
            }

            fn created_at(&self) -> ::chrono::DateTime<::chrono::Utc> {
                self.created_at
            }

            fn updated_at(&self) -> ::chrono::DateTime<::chrono::Utc> {
                self.updated_at
            }

            fn deleted_at(&self) -> Option<::chrono::DateTime<::chrono::Utc>> {
                self.deleted_at
            }

            fn status(&self) -> &str {
                &self.status
            }
        }

        // Implement Link trait
        impl $crate::core::entity::Link for $type {
            fn source_id(&self) -> ::uuid::Uuid {
                self.source_id
            }

            fn target_id(&self) -> ::uuid::Uuid {
                self.target_id
            }

            fn link_type(&self) -> &str {
                &self.link_type
            }
        }

        // Utility methods
        impl $type {
            /// Create a new link instance
            pub fn new(
                link_type: String,
                source_id: ::uuid::Uuid,
                target_id: ::uuid::Uuid,
                status: String,
                $( $specific_field: $specific_type ),*
            ) -> Self {
                Self {
                    id: ::uuid::Uuid::new_v4(),
                    entity_type: $type_name.to_string(),
                    created_at: ::chrono::Utc::now(),
                    updated_at: ::chrono::Utc::now(),
                    deleted_at: None,
                    status,
                    link_type,
                    source_id,
                    target_id,
                    $( $specific_field ),*
                }
            }

            /// Soft delete this link
            pub fn soft_delete(&mut self) {
                self.deleted_at = Some(::chrono::Utc::now());
                self.updated_at = ::chrono::Utc::now();
            }

            /// Restore a soft-deleted link
            #[allow(dead_code)]
            pub fn restore(&mut self) {
                self.deleted_at = None;
                self.updated_at = ::chrono::Utc::now();
            }

            /// Update the updated_at timestamp
            #[allow(dead_code)]
            pub fn touch(&mut self) {
                self.updated_at = ::chrono::Utc::now();
            }

            /// Change the link status
            #[allow(dead_code)]
            pub fn set_status(&mut self, status: String) {
                self.status = status;
                self.touch();
            }
        }
    };
}

/// Extended macro to create a Data entity with validation and filtering
///
/// This macro extends `impl_data_entity!` with declarative validation and filtering support.
///
/// # Example
///
/// ```rust,ignore
/// use this::prelude::*;
///
/// impl_data_entity_validated!(
///     Invoice,
///     "invoice",
///     ["name", "number"],
///     {
///         number: String,
///         amount: f64,
///         due_date: Option<String>,
///     },
///     validate: {
///         create: {
///             number: [required, string_length(3, 50)],
///             amount: [required, positive],
///         },
///         update: {
///             amount: [optional, positive],
///         },
///     },
///     filters: {
///         create: {
///             number: [trim, uppercase],
///             amount: [round_decimals(2)],
///         },
///     }
/// );
/// ```
#[macro_export]
macro_rules! impl_data_entity_validated {
    (
        $type:ident,
        $type_name:expr,
        [ $( $indexed_field:expr ),* $(,)? ],
        {
            $( $specific_field:ident : $specific_type:ty ),* $(,)?
        }
        $(,)?
        validate: {
            $(
                $op:ident: {
                    $(
                        $val_field:ident: [ $( $validator:tt )* ]
                    ),* $(,)?
                }
            ),* $(,)?
        }
        $(,)?
        filters: {
            $(
                $fop:ident: {
                    $(
                        $fil_field:ident: [ $( $filter:tt )* ]
                    ),* $(,)?
                }
            ),* $(,)?
        }
        $(,)?
    ) => {
        // 1. Generate the base entity (reuse existing macro)
        $crate::impl_data_entity!(
            $type,
            $type_name,
            [ $( $indexed_field ),* ],
            {
                $( $specific_field : $specific_type ),*
            }
        );

        // 2. Implement ValidatableEntity trait for validation support
        impl $crate::core::validation::extractor::ValidatableEntity for $type {
            fn validation_config(operation: &str) -> $crate::core::validation::EntityValidationConfig {
                use $crate::core::validation::*;

                let mut config = EntityValidationConfig::new($type_name);

                // Generate validation rules per operation
                $(
                    if operation == stringify!($op) {
                        $(
                            // Add validators for each field
                            $crate::add_validators_for_field!(config, stringify!($val_field), $( $validator )*);
                        )*
                    }
                )*

                // Generate filters per operation
                $(
                    if operation == stringify!($fop) {
                        $(
                            // Add filters for each field
                            $crate::add_filters_for_field!(config, stringify!($fil_field), $( $filter )*);
                        )*
                    }
                )*

                config
            }
        }
    };
}

/// Helper macro to add validators to a field
#[macro_export]
macro_rules! add_validators_for_field {
    // Base case: empty
    ($config:expr, $field:expr,) => {};

    // required
    ($config:expr, $field:expr, required $( $rest:tt )*) => {
        $config.add_validator($field, $crate::core::validation::validators::required());
        $crate::add_validators_for_field!($config, $field, $( $rest )*);
    };

    // optional
    ($config:expr, $field:expr, optional $( $rest:tt )*) => {
        $config.add_validator($field, $crate::core::validation::validators::optional());
        $crate::add_validators_for_field!($config, $field, $( $rest )*);
    };

    // positive
    ($config:expr, $field:expr, positive $( $rest:tt )*) => {
        $config.add_validator($field, $crate::core::validation::validators::positive());
        $crate::add_validators_for_field!($config, $field, $( $rest )*);
    };

    // string_length with parameters
    ($config:expr, $field:expr, string_length($min:expr, $max:expr) $( $rest:tt )*) => {
        $config.add_validator($field, $crate::core::validation::validators::string_length($min, $max));
        $crate::add_validators_for_field!($config, $field, $( $rest )*);
    };

    // max_value with parameter
    ($config:expr, $field:expr, max_value($max:expr) $( $rest:tt )*) => {
        $config.add_validator($field, $crate::core::validation::validators::max_value($max));
        $crate::add_validators_for_field!($config, $field, $( $rest )*);
    };

    // in_list with values
    ($config:expr, $field:expr, in_list($( $value:expr ),* $(,)?) $( $rest:tt )*) => {
        $config.add_validator($field, $crate::core::validation::validators::in_list(vec![$( $value.to_string() ),*]));
        $crate::add_validators_for_field!($config, $field, $( $rest )*);
    };

    // date_format with format string
    ($config:expr, $field:expr, date_format($format:expr) $( $rest:tt )*) => {
        $config.add_validator($field, $crate::core::validation::validators::date_format($format));
        $crate::add_validators_for_field!($config, $field, $( $rest )*);
    };
}

/// Helper macro to add filters to a field
#[macro_export]
macro_rules! add_filters_for_field {
    // Base case: empty
    ($config:expr, $field:expr,) => {};

    // trim
    ($config:expr, $field:expr, trim $( $rest:tt )*) => {
        $config.add_filter($field, $crate::core::validation::filters::trim());
        $crate::add_filters_for_field!($config, $field, $( $rest )*);
    };

    // uppercase
    ($config:expr, $field:expr, uppercase $( $rest:tt )*) => {
        $config.add_filter($field, $crate::core::validation::filters::uppercase());
        $crate::add_filters_for_field!($config, $field, $( $rest )*);
    };

    // lowercase
    ($config:expr, $field:expr, lowercase $( $rest:tt )*) => {
        $config.add_filter($field, $crate::core::validation::filters::lowercase());
        $crate::add_filters_for_field!($config, $field, $( $rest )*);
    };

    // round_decimals with parameter
    ($config:expr, $field:expr, round_decimals($decimals:expr) $( $rest:tt )*) => {
        $config.add_filter($field, $crate::core::validation::filters::round_decimals($decimals));
        $crate::add_filters_for_field!($config, $field, $( $rest )*);
    };
}

#[cfg(test)]
mod tests {
    use crate::prelude::*;

    // Test Data entity
    impl_data_entity!(
        TestUser,
        "test_user",
        ["name", "email"],
        {
            email: String,
        }
    );

    // Test Link entity
    impl_link_entity!(
        TestOwnerLink,
        "test_owner_link",
        {
            since: DateTime<Utc>,
        }
    );

    #[test]
    fn test_data_entity_creation() {
        let user = TestUser::new(
            "John Doe".to_string(),
            "active".to_string(),
            "john@example.com".to_string(),
        );

        assert_eq!(user.name(), "John Doe");
        assert_eq!(user.status(), "active");
        assert_eq!(user.email, "john@example.com");
        assert!(!user.is_deleted());
        assert!(user.is_active());
    }

    #[test]
    fn test_data_entity_soft_delete() {
        let mut user = TestUser::new(
            "John Doe".to_string(),
            "active".to_string(),
            "john@example.com".to_string(),
        );

        assert!(!user.is_deleted());
        user.soft_delete();
        assert!(user.is_deleted());
        assert!(!user.is_active());
    }

    #[test]
    fn test_data_entity_restore() {
        let mut user = TestUser::new(
            "John Doe".to_string(),
            "active".to_string(),
            "john@example.com".to_string(),
        );

        user.soft_delete();
        assert!(user.is_deleted());

        user.restore();
        assert!(!user.is_deleted());
        assert!(user.is_active());
    }

    #[test]
    fn test_link_entity_creation() {
        let user_id = Uuid::new_v4();
        let car_id = Uuid::new_v4();

        let link = TestOwnerLink::new(
            "owner".to_string(),
            user_id,
            car_id,
            "active".to_string(),
            Utc::now(),
        );

        assert_eq!(link.source_id(), user_id);
        assert_eq!(link.target_id(), car_id);
        assert_eq!(link.link_type(), "owner");
        assert_eq!(link.status(), "active");
        assert!(!link.is_deleted());
    }

    #[test]
    fn test_link_entity_soft_delete() {
        let link = TestOwnerLink::new(
            "owner".to_string(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            "active".to_string(),
            Utc::now(),
        );

        let mut link = link;
        assert!(!link.is_deleted());

        link.soft_delete();
        assert!(link.is_deleted());
    }

    #[test]
    fn test_entity_set_status() {
        let mut user = TestUser::new(
            "John Doe".to_string(),
            "active".to_string(),
            "john@example.com".to_string(),
        );

        assert_eq!(user.status(), "active");

        user.set_status("inactive".to_string());
        assert_eq!(user.status(), "inactive");
    }
}
