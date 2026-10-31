//! Macros for reducing boilerplate when defining entities
//!
//! These macros generate the repetitive trait implementations needed
//! for each entity type following the Entity/Data/Link architecture.

/// Helper macro to read the tenant of a multi-tenant entity
///
/// # ⚠️ Cette macro n'active PAS le multi-tenant (2026-10-01)
///
/// Elle documentait « ajoute un override de `Entity::tenant_id()` » : c'est
/// **faux**, et le commentaire intérieur le disait déjà — on ne peut pas
/// ajouter un override de trait dans un `impl` séparé en Rust stable, et la
/// macro ne peut pas non plus écrire le `impl Entity` (celui que
/// [`impl_data_entity!`](crate::impl_data_entity) génère déjà).
///
/// Ce qu'elle fait donc, exactement : une méthode **inhérente**
/// `get_tenant_id()` qui lit le champ `tenant_id`. Utile pour lire le champ ;
/// **sans effet sur le framework**, qui lit `Entity::tenant_id()` — dont le
/// défaut est `None`.
///
/// # Ce qu'il faut écrire pour activer réellement le multi-tenant
///
/// L'override vit **dans votre `impl Entity`** (ou dans une méthode du même
/// bloc) :
///
/// ```rust,ignore
/// impl Entity for User {
///     fn tenant_id(&self) -> Option<Uuid> {
///         Some(self.tenant_id)   // ← ce que le FRAMEWORK lit
///     }
/// }
/// ```
///
/// Le test `multi_tenant_macro_only_reads_the_field_not_the_trait` consigne
/// l'écart : il montre qu'après `impl_entity_multi_tenant!(E)`, le trait répond
/// toujours `None`. Il **tombera** le jour où la macro câblera réellement le
/// trait — et il faudra alors réécrire cette documentation, pas la laisser
/// mentir.
///
/// # Example
/// ```rust,ignore
/// impl_data_entity!(User, "user", ["name"], {
///     tenant_id: Uuid,
///     email: String,
/// });
///
/// // Lire le champ :
/// impl_entity_multi_tenant!(User);
///
/// // Et pour que le FRAMEWORK le voie, l'override de trait reste à écrire :
/// impl Entity for User {
///     fn tenant_id(&self) -> Option<Uuid> { Some(self.tenant_id) }
/// }
/// ```
#[macro_export]
macro_rules! impl_entity_multi_tenant {
    ($type:ident) => {
        // Ne PAS écrire ici « active le multi-tenant » : cette macro ne peut pas
        // override un trait dans un `impl` séparé (Rust stable), et le `impl
        // Entity` appartient à `impl_data_entity!`. Elle lit le champ, rien de
        // plus — voir la documentation ci-dessus, qui dit où vit l'override.
        impl $type {
            /// Lire le champ `tenant_id` — **ce que le framework ne lit pas**.
            ///
            /// Pour que la couche de données et le confinement par locataire le
            /// voient, écrivez `fn tenant_id(&self) -> Option<Uuid>` dans votre
            /// `impl Entity`.
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
        $crate::impl_data_entity!(
            @entity $type, $type_name, [ $( $indexed_field ),* ], { $( $specific_field : $specific_type ),* }
        );
        $crate::__this_described!(
            $type, $type_name, [ $( $indexed_field ),* ], { $( $specific_field : $specific_type ),* }, { }
        );
    };
    // Internal: the entity without its `Described` impl (used by `impl_data_entity_validated!`,
    // which emits a `Described` carrying its rules).
    (
        @entity
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
            type Service = ();

            fn resource_name() -> &'static str {
                use std::sync::OnceLock;
                static PLURAL: OnceLock<&'static str> = OnceLock::new();
                PLURAL.get_or_init(|| {
                    Box::leak(
                        $crate::core::pluralize::Pluralizer::pluralize($type_name)
                            .into_boxed_str()
                    )
                })
            }

            fn resource_name_singular() -> &'static str {
                $type_name
            }

            fn service_from_host(
                _host: &::std::sync::Arc<dyn ::std::any::Any + Send + Sync>
            ) -> ::anyhow::Result<::std::sync::Arc<Self::Service>> {
                // **Un refus DIAGNOSTIQUABLE, pas une panique.** Cette macro ne
                // peut pas deviner le service de VOTRE entite : elle genere donc
                // `type Service = ()` et cette fonction, qui rend une erreur
                // nommant le type et le remede. Avant, elle faisait
                // `unimplemented!()` — un processus qui s'arrete sans dire quoi
                // ecrire, la ou un `Err` remonte jusqu'a l'appelant qui peut le
                // lire (et le journaliser). Un service par entite s'ecrit dans
                // un `impl Entity` a vous ; sinon, `()` suffit et cette fonction
                // n'a pas a etre appelee.
                ::anyhow::bail!(
                    "service_from_host n'est pas implemente pour `{}` : la macro genere \
                     `type Service = ()` mais ne peut pas deviner le service de votre entite. \
                     Ecrivez `impl Entity for {}` avec `service_from_host`, ou n'attachez pas \
                     de service par entite (le service rendu est `()`).",
                    stringify!($type),
                    stringify!($type)
                )
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

            /// The type carries `deleted_at`, so the mark is recorded here and
            /// the caller is told it worked. `updated_at` follows, because the
            /// mark is a write like any other.
            fn set_deleted_at(
                &mut self,
                at: Option<::chrono::DateTime<::chrono::Utc>>,
            ) -> bool {
                self.deleted_at = at;
                self.updated_at = ::chrono::Utc::now();
                true
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
            type Service = ();

            fn resource_name() -> &'static str {
                use std::sync::OnceLock;
                static PLURAL: OnceLock<&'static str> = OnceLock::new();
                PLURAL.get_or_init(|| {
                    Box::leak(
                        $crate::core::pluralize::Pluralizer::pluralize($type_name)
                            .into_boxed_str()
                    )
                })
            }

            fn resource_name_singular() -> &'static str {
                $type_name
            }

            fn service_from_host(
                _host: &::std::sync::Arc<dyn ::std::any::Any + Send + Sync>
            ) -> ::anyhow::Result<::std::sync::Arc<Self::Service>> {
                // **Un refus DIAGNOSTIQUABLE, pas une panique.** Cette macro ne
                // peut pas deviner le service de VOTRE entite : elle genere donc
                // `type Service = ()` et cette fonction, qui rend une erreur
                // nommant le type et le remede. Avant, elle faisait
                // `unimplemented!()` — un processus qui s'arrete sans dire quoi
                // ecrire, la ou un `Err` remonte jusqu'a l'appelant qui peut le
                // lire (et le journaliser). Un service par entite s'ecrit dans
                // un `impl Entity` a vous ; sinon, `()` suffit et cette fonction
                // n'a pas a etre appelee.
                ::anyhow::bail!(
                    "service_from_host n'est pas implemente pour `{}` : la macro genere \
                     `type Service = ()` mais ne peut pas deviner le service de votre entite. \
                     Ecrivez `impl Entity for {}` avec `service_from_host`, ou n'attachez pas \
                     de service par entite (le service rendu est `()`).",
                    stringify!($type),
                    stringify!($type)
                )
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

            /// The type carries `deleted_at`, so the mark is recorded here and
            /// the caller is told it worked. `updated_at` follows, because the
            /// mark is a write like any other.
            fn set_deleted_at(
                &mut self,
                at: Option<::chrono::DateTime<::chrono::Utc>>,
            ) -> bool {
                self.deleted_at = at;
                self.updated_at = ::chrono::Utc::now();
                true
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
            @entity
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

        // 3. Module descriptor, carrying the same rules (additive: the legacy
        //    `ValidatableEntity` above is untouched).
        $crate::__this_described!(
            $type, $type_name, [ $( $indexed_field ),* ], { $( $specific_field : $specific_type ),* },
            {
                validate: { $( $op: { $( $val_field: [ $( $validator )* ] ),* } ),* }
                filters: { $( $fop: { $( $fil_field: [ $( $filter )* ] ),* } ),* }
            }
        );
    };
}

/// Emit `impl Described` (module descriptor) for an entity generated by
/// `impl_data_entity!` / `impl_data_entity_validated!`, plus the inherent
/// `Type::descriptor()`.
///
/// - Field types come from [`FieldTypeOf`](crate::descriptor::FieldTypeOf) through an
///   autoref probe: a type without an impl is described as `json` (it still compiles).
/// - `Option<T>` gives `optional: true`; a field named `tenant_id` is `read_only`.
/// - The plural is `Pluralizer::pluralize(type_name)`, the same function as
///   `Entity::resource_name()`: one source.
/// - No proto tag is assigned (tags come from `descriptor.lock` or explicit declarations,
///   never from declaration order).
/// - Built once (`OnceLock`).
#[doc(hidden)]
#[macro_export]
macro_rules! __this_described {
    (
        $type:ident,
        $type_name:expr,
        [ $( $indexed_field:expr ),* ],
        { $( $specific_field:ident : $specific_type:ty ),* },
        { $( $rules:tt )* }
    ) => {
        impl $crate::descriptor::Described for $type {
            fn entity_desc() -> &'static $crate::descriptor::EntityDesc {
                static DESC: ::std::sync::OnceLock<$crate::descriptor::EntityDesc> =
                    ::std::sync::OnceLock::new();
                DESC.get_or_init(|| {
                    #[allow(unused_imports)]
                    use $crate::descriptor::field::{ProbeFallback as _, ProbeKnown as _};
                    #[allow(unused_mut)]
                    let mut e = $crate::descriptor::EntityDesc::new($type_name);
                    e.indexed = ::std::vec![ $( ::std::string::ToString::to_string(&$indexed_field) ),* ];
                    $(
                        {
                            let (ty, optional) =
                                (&&$crate::descriptor::field::FieldProbe::<$specific_type>::new())
                                    .probe_field_type();
                            let mut f = $crate::descriptor::FieldDesc::new(stringify!($specific_field), ty);
                            f.optional = optional;
                            if f.name == $crate::descriptor::entity::TENANT_ID {
                                f.read_only = true;
                            }
                            e.fields.push(f);
                        }
                    )*
                    $crate::__this_desc_rules!(e, $( $rules )*);
                    e.normalize();
                    e
                })
            }
        }

        impl $type {
            /// Descriptor of this entity (same as `<Self as Described>::entity_desc()`).
            #[allow(dead_code)]
            pub fn descriptor() -> &'static $crate::descriptor::EntityDesc {
                <Self as $crate::descriptor::Described>::entity_desc()
            }
        }
    };
}

/// Translate the `validate:` / `filters:` grammar into descriptor rules.
#[doc(hidden)]
#[macro_export]
macro_rules! __this_desc_rules {
    ($e:ident, ) => {};
    (
        $e:ident,
        validate: { $( $op:ident: { $( $val_field:ident: [ $( $validator:tt )* ] ),* } ),* }
        filters: { $( $fop:ident: { $( $fil_field:ident: [ $( $filter:tt )* ] ),* } ),* }
    ) => {
        $(
            {
                #[allow(unused_variables)]
                let on = $crate::__this_applies!($op);
                $(
                    {
                        let field = $crate::__this_overlay!($e, stringify!($val_field));
                        $crate::__this_validator_specs!(field, on, $( $validator )*);
                    }
                )*
            }
        )*
        $(
            {
                #[allow(unused_variables)]
                let on = $crate::__this_applies!($fop);
                $(
                    {
                        let field = $crate::__this_overlay!($e, stringify!($fil_field));
                        $crate::__this_transform_specs!(field, on, $( $filter )*);
                    }
                )*
            }
        )*
    };
}

/// `create` / `update` only. Any other operation name is a compile error (the legacy
/// grammar silently ignored a typo such as `creat:`).
#[doc(hidden)]
#[macro_export]
macro_rules! __this_applies {
    (create) => {
        $crate::descriptor::Applies::Create
    };
    (update) => {
        $crate::descriptor::Applies::Update
    };
    ($other:ident) => {
        compile_error!(concat!(
            "impl_data_entity_validated!: unknown operation `",
            stringify!($other),
            "` (expected `create` or `update`)"
        ))
    };
}

/// Declared field, or overlay of a writable base field (`name`, `status`).
#[doc(hidden)]
#[macro_export]
macro_rules! __this_overlay {
    ($e:ident, $name:expr) => {
        match $e.field_or_overlay_mut($name) {
            ::std::option::Option::Some(f) => f,
            ::std::option::Option::None => panic!(
                "impl_data_entity_validated!({}): rule on `{}`, which is neither a declared \
                 field nor a writable base field (name, status)",
                $e.name, $name
            ),
        }
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __this_validator_specs {
    ($f:ident, $on:ident, ) => {};
    ($f:ident, $on:ident, required $( $rest:tt )*) => {
        $f.validators.push($crate::descriptor::ValidatorRule { rule: $crate::descriptor::ValidatorSpec::Required, on: $on });
        $crate::__this_validator_specs!($f, $on, $( $rest )*);
    };
    ($f:ident, $on:ident, optional $( $rest:tt )*) => {
        $f.validators.push($crate::descriptor::ValidatorRule { rule: $crate::descriptor::ValidatorSpec::Optional, on: $on });
        $crate::__this_validator_specs!($f, $on, $( $rest )*);
    };
    ($f:ident, $on:ident, positive $( $rest:tt )*) => {
        $f.validators.push($crate::descriptor::ValidatorRule { rule: $crate::descriptor::ValidatorSpec::Positive, on: $on });
        $crate::__this_validator_specs!($f, $on, $( $rest )*);
    };
    ($f:ident, $on:ident, string_length($min:expr, $max:expr) $( $rest:tt )*) => {
        $f.validators.push($crate::descriptor::ValidatorRule {
            rule: $crate::descriptor::ValidatorSpec::StringLength { min: $min, max: $max },
            on: $on,
        });
        $crate::__this_validator_specs!($f, $on, $( $rest )*);
    };
    ($f:ident, $on:ident, max_value($max:expr) $( $rest:tt )*) => {
        $f.validators.push($crate::descriptor::ValidatorRule {
            rule: $crate::descriptor::ValidatorSpec::max_value_f64(($max) as f64),
            on: $on,
        });
        $crate::__this_validator_specs!($f, $on, $( $rest )*);
    };
    ($f:ident, $on:ident, in_list($( $value:expr ),* $(,)?) $( $rest:tt )*) => {
        $f.validators.push($crate::descriptor::ValidatorRule {
            rule: $crate::descriptor::ValidatorSpec::in_list_strings(::std::vec![$( ::std::string::ToString::to_string(&$value) ),*]),
            on: $on,
        });
        $crate::__this_validator_specs!($f, $on, $( $rest )*);
    };
    ($f:ident, $on:ident, date_format($format:expr) $( $rest:tt )*) => {
        $f.validators.push($crate::descriptor::ValidatorRule {
            rule: $crate::descriptor::ValidatorSpec::DateFormat(::std::string::ToString::to_string(&$format)),
            on: $on,
        });
        $crate::__this_validator_specs!($f, $on, $( $rest )*);
    };
}

#[doc(hidden)]
#[macro_export]
macro_rules! __this_transform_specs {
    ($f:ident, $on:ident, ) => {};
    ($f:ident, $on:ident, trim $( $rest:tt )*) => {
        $f.transforms.push($crate::descriptor::TransformRule { rule: $crate::descriptor::TransformSpec::Trim, on: $on });
        $crate::__this_transform_specs!($f, $on, $( $rest )*);
    };
    ($f:ident, $on:ident, uppercase $( $rest:tt )*) => {
        $f.transforms.push($crate::descriptor::TransformRule { rule: $crate::descriptor::TransformSpec::Uppercase, on: $on });
        $crate::__this_transform_specs!($f, $on, $( $rest )*);
    };
    ($f:ident, $on:ident, lowercase $( $rest:tt )*) => {
        $f.transforms.push($crate::descriptor::TransformRule { rule: $crate::descriptor::TransformSpec::Lowercase, on: $on });
        $crate::__this_transform_specs!($f, $on, $( $rest )*);
    };
    ($f:ident, $on:ident, round_decimals($decimals:expr) $( $rest:tt )*) => {
        $f.transforms.push($crate::descriptor::TransformRule {
            rule: $crate::descriptor::TransformSpec::RoundDecimals($decimals),
            on: $on,
        });
        $crate::__this_transform_specs!($f, $on, $( $rest )*);
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
        $config.add_validator($field, $crate::core::validation::validators::max_value(($max) as f64));
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

    /// The generated `Entity::set_deleted_at` — the primitive the soft-delete
    /// decorator relies on. It answers `true` (the type carries the field) and
    /// `updated_at` follows the mark, because the mark is a write like any other.
    /// This is the only test that exercises the *trait* method rather than the
    /// inherent `soft_delete()`, which is why the generated code was uncovered.
    #[test]
    fn test_data_entity_set_deleted_at_through_the_trait() {
        use crate::core::Entity as _;

        let mut user = TestUser::new(
            "John Doe".to_string(),
            "active".to_string(),
            "john@example.com".to_string(),
        );
        let before = user.updated_at();

        assert!(user.set_deleted_at(Some(chrono::Utc::now())));
        assert!(user.is_deleted());
        assert!(user.updated_at() >= before, "the mark touches updated_at");

        // Reinstate through the same primitive.
        assert!(user.set_deleted_at(None));
        assert!(!user.is_deleted());
        assert!(user.deleted_at().is_none());
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

    /// Same primitive on a link entity: the trait method, not the inherent one.
    #[test]
    fn test_link_entity_set_deleted_at_through_the_trait() {
        use crate::core::Entity as _;

        let mut link = TestOwnerLink::new(
            "owner".to_string(),
            Uuid::new_v4(),
            Uuid::new_v4(),
            "active".to_string(),
            Utc::now(),
        );

        assert!(link.set_deleted_at(Some(Utc::now())));
        assert!(link.is_deleted());
        assert!(link.set_deleted_at(None));
        assert!(!link.is_deleted());
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

    #[test]
    fn test_data_entity_field_value_name() {
        let user = TestUser::new(
            "Alice".to_string(),
            "active".to_string(),
            "alice@example.com".to_string(),
        );

        let name_val = user
            .field_value("name")
            .expect("field_value('name') should return Some");
        assert_eq!(
            name_val,
            crate::core::field::FieldValue::String("Alice".to_string())
        );
    }

    #[test]
    fn test_data_entity_field_value_status() {
        let user = TestUser::new(
            "Bob".to_string(),
            "pending".to_string(),
            "bob@example.com".to_string(),
        );

        let status_val = user
            .field_value("status")
            .expect("field_value('status') should return Some");
        assert_eq!(
            status_val,
            crate::core::field::FieldValue::String("pending".to_string())
        );
    }

    #[test]
    fn test_data_entity_field_value_unknown_returns_none() {
        let user = TestUser::new(
            "Charlie".to_string(),
            "active".to_string(),
            "charlie@example.com".to_string(),
        );

        // "email" is a custom field but the default field_value() macro only handles "name" and "status"
        assert!(user.field_value("email").is_none());
        assert!(user.field_value("nonexistent").is_none());
    }

    #[test]
    fn test_data_entity_resource_name() {
        assert_eq!(TestUser::resource_name(), "test_users");
    }

    #[test]
    fn test_data_entity_resource_name_singular() {
        assert_eq!(TestUser::resource_name_singular(), "test_user");
    }

    #[test]
    fn test_link_entity_resource_name() {
        assert_eq!(TestOwnerLink::resource_name(), "test_owner_links");
        assert_eq!(TestOwnerLink::resource_name_singular(), "test_owner_link");
    }

    #[test]
    fn test_data_entity_indexed_fields() {
        let fields = TestUser::indexed_fields();
        assert!(fields.contains(&"name"));
        assert!(fields.contains(&"email"));
        assert_eq!(fields.len(), 2);
    }

    // ======================================================================
    // Le trou du contrat d'entité : `service_from_host` et le multi-tenant
    // ======================================================================

    /// La macro **ne peut pas** deviner le service de l'entité : elle doit donc
    /// le **dire**, pas s'arrêter net.
    ///
    /// Avant, la fonction générée faisait `unimplemented!()` : un processus qui
    /// meurt sans laisser quoi écrire, là où la signature promet un `Result`.
    /// Le refus porte désormais le type **et** le geste à écrire — il remonte
    /// jusqu'à l'appelant, qui peut le journaliser.
    #[test]
    fn test_service_from_host_non_implemente_dit_quoi_ecrire() {
        use std::sync::Arc;

        let host: Arc<dyn std::any::Any + Send + Sync> = Arc::new(());

        let erreur = TestUser::service_from_host(&host)
            .expect_err("la macro ne peut pas deviner le service : elle doit le REFUSER");
        let message = erreur.to_string();
        assert!(
            message.contains("TestUser"),
            "l'erreur nomme le type fautif : {message}"
        );
        assert!(
            message.contains("service_from_host"),
            "l'erreur nomme le geste à écrire : {message}"
        );

        // Le lien aussi, et pas seulement l'entité de données.
        let erreur_lien = TestOwnerLink::service_from_host(&host)
            .expect_err("même refus pour les entités de lien");
        assert!(erreur_lien.to_string().contains("TestOwnerLink"));
    }

    // Une entité multi-tenant, telle que la documentation la décrit.
    impl_data_entity!(
        TestTenantUser,
        "test_tenant_user",
        ["name"],
        {
            tenant_id: Uuid,
        }
    );
    crate::impl_entity_multi_tenant!(TestTenantUser);

    /// **`impl_entity_multi_tenant!` n'active pas le multi-tenant** — et ce test
    /// consigne l'écart au lieu de laisser la documentation le promettre.
    ///
    /// La macro ne peut pas surcharger `Entity::tenant_id()` (un `impl` de trait
    /// séparé, en Rust stable, n'existe pas) : elle lit le champ, rien de plus.
    /// Le framework, lui, lit le **trait**, dont le défaut est `None`.
    ///
    /// Ce test **tombera** le jour où la macro câblera réellement le trait — et
    /// ce jour-là, c'est la documentation de la macro qu'il faudra réécrire, pas
    /// ce test qu'il faudra faire taire.
    #[test]
    fn test_multi_tenant_macro_lit_le_champ_mais_pas_le_trait() {
        use crate::core::entity::Entity;

        let locataire = Uuid::new_v4();
        let mut utilisateur =
            TestTenantUser::new("Jane".to_string(), "active".to_string(), locataire);

        assert_eq!(
            utilisateur.get_tenant_id(),
            locataire,
            "le helper de la macro LIT bien le champ"
        );
        assert_eq!(
            Entity::tenant_id(&utilisateur),
            None,
            "mais le FRAMEWORK lit le trait, dont le défaut est `None` : l'override \
             (`fn tenant_id(&self) -> Option<Uuid> {{ Some(self.tenant_id) }}`) reste à \
             écrire dans `impl Entity` — la macro ne le fait pas et ne peut pas le faire"
        );

        // Le GESTE de suppression douce, lui, est bien engendré — et il n'est
        // filtré par rien : le framework marque et rend, l'application cache
        // (voir `Entity::deleted_at`).
        utilisateur.soft_delete();
        assert!(utilisateur.is_deleted());
        utilisateur.restore();
        assert!(!utilisateur.is_deleted());
        utilisateur.set_status("archive".to_string());
        assert_eq!(utilisateur.status(), "archive");
    }
}

#[cfg(test)]
mod descriptor_macro_tests {
    // The generated `new` / `soft_delete` helpers are not exercised here.
    #![allow(dead_code)]
    use crate::descriptor::{
        Applies, CheckProfile, CustomRegistry, Described, FieldType, ModuleDescriptor,
        TransformSpec, ValidatorSpec,
    };
    #[allow(unused_imports)]
    use crate::prelude::*;

    impl_data_entity!(Knife, "knife", ["name"], {
        price: f64,
        stock: Option<u32>,
        tenant_id: uuid::Uuid,
        extra: std::collections::HashMap<String, String>,
    });

    impl_data_entity_validated!(
        Invoice,
        "invoice",
        ["name", "number"],
        {
            number: String,
            amount: f64,
            due_date: Option<String>,
        },
        validate: {
            create: {
                number: [required string_length(3, 50)],
                amount: [required positive max_value(1_000_000.0)],
                status: [required in_list("draft", "sent")],
            },
            update: {
                amount: [optional positive],
            },
        },
        filters: {
            create: {
                number: [trim uppercase],
                amount: [round_decimals(2)],
            },
        }
    );

    #[test]
    fn macro_emits_described_types_and_plural() {
        let d = Knife::entity_desc();
        assert!(std::ptr::eq(d, Knife::descriptor()), "built once");
        assert_eq!(
            d.plural,
            <Knife as crate::core::entity::Entity>::resource_name()
        );
        assert_eq!(d.plural, "knives");
        let price = d.field("price").unwrap();
        assert_eq!((price.ty.clone(), price.optional), (FieldType::F64, false));
        let stock = d.field("stock").unwrap();
        assert_eq!((stock.ty.clone(), stock.optional), (FieldType::U32, true));
        assert!(d.field("tenant_id").unwrap().read_only);
        // Type without FieldTypeOf: still compiles, described as json.
        assert_eq!(d.field("extra").unwrap().ty, FieldType::Json);
        assert!(
            d.fields.iter().all(|f| f.proto_tag.is_none()),
            "macro never assigns tags"
        );
    }

    impl_data_entity_validated!(
        Person,
        "person",
        ["name"],
        {
            age: i32,
        },
        validate: {
            create: {
                age: [required max_value(150)],
            },
            update: {},
        },
        filters: {
            create: {},
        }
    );

    /// `max_value(150)` on an INTEGER field: the bound must stay an integer in the descriptor
    /// (it used to become the float 150.0, refused by `check`).
    #[test]
    fn macro_max_value_integer_bound_on_integer_field() {
        let d = Person::entity_desc();
        let age = d.field("age").unwrap();
        assert!(
            age.validators
                .iter()
                .any(|r| matches!(&r.rule, ValidatorSpec::MaxValue(n) if n.is_i64()))
        );
        let mut m = ModuleDescriptor::new("people", "1.0.0");
        m.entities.push(d.clone());
        m.check(&CustomRegistry::default(), CheckProfile::Compiled)
            .unwrap();
    }

    #[test]
    fn macro_emits_described_with_validators() {
        let d = Invoice::entity_desc();
        let amount = d.field("amount").unwrap();
        // `positive` on create AND update merged into one `Always` rule.
        assert!(
            amount
                .validators
                .iter()
                .any(|r| r.rule == ValidatorSpec::Positive && r.on == Applies::Always)
        );
        assert!(
            amount
                .validators
                .iter()
                .any(|r| r.rule == ValidatorSpec::Required && r.on == Applies::Create)
        );
        assert_eq!(amount.transforms[0].rule, TransformSpec::RoundDecimals(2));
        // Rule on the base field `status`: carried by an overlay.
        let status = d.field("status").unwrap();
        assert_eq!(status.ty, FieldType::String);
        assert!(matches!(
            status.validators[1].rule,
            ValidatorSpec::InList(_)
        ));
        // The descriptor is valid.
        let mut m = ModuleDescriptor::new("billing", "1.0.0");
        m.entities.push(d.clone());
        m.check(&CustomRegistry::default(), CheckProfile::Compiled)
            .unwrap();
        // Legacy ValidatableEntity still works.
        use crate::core::validation::extractor::ValidatableEntity;
        let _ = Invoice::validation_config("create");
    }

    /// The macro assigns NO tag (RFC §3.3: declaration order would make them unstable);
    /// stable tags come from the lock: `assign_tags` gives user fields 16.. in NAME order,
    /// and a later declaration reorder keeps them.
    #[test]
    fn macro_entity_gets_stable_tags_from_lock_starting_at_16() {
        use crate::descriptor::derive::proto::{TagMode, assign_tags, entity_proto, tag_map};
        let mut d = Invoice::entity_desc().clone();
        assert!(
            entity_proto(&d, TagMode::Strict).is_err(),
            "untagged: strict refuses"
        );
        assign_tags(&mut d, &Default::default());
        let lock = tag_map(&d);
        assert_eq!(lock["amount"], 16);
        assert_eq!(lock["due_date"], 17);
        assert_eq!(lock["number"], 18);
        assert_eq!(
            lock["status"], 6,
            "overlay of a base field keeps its base tag"
        );
        let proto = entity_proto(&d, TagMode::Strict).unwrap();
        // Same lock applied to the fields in another order: same .proto.
        let mut r = Invoice::entity_desc().clone();
        r.fields.reverse();
        assign_tags(&mut r, &lock);
        assert_eq!(entity_proto(&r, TagMode::Strict).unwrap(), proto);
    }

    #[test]
    fn macro_descriptor_validates_like_legacy() {
        use crate::descriptor::engine::validate::{Side, ValidateCtx, WriteOp, apply_desc};
        let reg = CustomRegistry::default();
        let ctx = ValidateCtx {
            registry: &reg,
            side: Side::Server,
        };
        let ok = apply_desc(
            Invoice::entity_desc(),
            WriteOp::Create,
            serde_json::json!({"name": "i", "status": "draft", "number": " ab1 ", "amount": 10.556}),
            &ctx,
        )
        .unwrap();
        assert_eq!(ok.value["number"], "AB1");
        assert_eq!(ok.value["amount"], 10.56);
        let err = apply_desc(
            Invoice::entity_desc(),
            WriteOp::Create,
            serde_json::json!({"number": "abc", "amount": -1, "status": "draft"}),
            &ctx,
        )
        .unwrap_err();
        assert_eq!(err.fields[0].code.as_str(), "this.validation.positive");
    }
}
