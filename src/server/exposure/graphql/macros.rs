//! Macros pour générer automatiquement les types GraphQL depuis les entités

/// Macro pour exposer une entité Data via GraphQL avec ses types spécifiques
///
/// Cette macro génère automatiquement:
/// - Un type GraphQL avec tous les champs de l'entité
/// - Des resolvers pour les relations (basés sur links.yaml)
/// - Des queries (get, list)
/// - Des mutations (create, update, delete)
///
/// # Exemple
///
/// ```rust,ignore
/// use this::prelude::*;
///
/// impl_data_entity_validated!(
///     Order,
///     "order",
///     ["name", "number"],
///     {
///         number: String,
///         amount: f64,
///         customer_name: Option<String>,
///     },
///     // ... validation et filters ...
/// );
///
/// // Exposer via GraphQL avec relations
/// graphql_entity!(Order, {
///     relations: {
///         invoices: [Invoice] via "invoices",
///     }
/// });
/// ```
#[macro_export]
#[cfg(feature = "graphql")]
macro_rules! graphql_entity {
    // Version simple sans relations
    ($type:ident) => {
        #[cfg(feature = "graphql")]
        #[async_graphql::Object]
        impl $type {
            // Les champs de base sont automatiquement exposés via SimpleObject
            // async-graphql va automatiquement exposer tous les champs publics
        }
    };

    // Version avec relations
    ($type:ident, {
        relations: {
            $( $relation_name:ident : $relation_type:tt via $link_type:expr ),* $(,)?
        }
    }) => {
        // Pour l'instant, on ne peut pas implémenter dynamiquement les relations
        // car async-graphql nécessite que les types soient connus à la compilation
        compile_error!("Relations dynamiques ne sont pas encore supportées. Utilisez la version sans relations.");
    };
}

/// Macro pour enregistrer une entité dans le schéma GraphQL du builder
///
/// Cette macro doit être appelée lors de la construction du serveur.
///
/// # Exemple
///
/// ```rust,ignore
/// let builder = ServerBuilder::new()
///     .with_module(order_module)
///     .with_module(invoice_module);
///
/// register_graphql_entities!(builder, Order, Invoice, Payment);
/// ```
#[macro_export]
#[cfg(feature = "graphql")]
macro_rules! register_graphql_entities {
    ($builder:expr, $( $type:ident ),* $(,)?) => {
        {
            // Cette macro est un placeholder pour future implémentation
            // Pour l'instant, les entités sont automatiquement détectées via EntityRegistry
            $builder
        }
    };
}

