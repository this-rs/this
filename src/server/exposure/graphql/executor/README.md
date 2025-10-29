# GraphQL Executor Module

Ce module implémente un exécuteur GraphQL personnalisé qui peut exécuter des requêtes et mutations contre notre schéma dynamique généré.

## 📁 Structure

L'exécuteur est organisé en plusieurs modules pour une meilleure maintenabilité :

```
executor/
├── mod.rs                  # Point d'entrée du module
├── core.rs                 # Orchestration principale (GraphQLExecutor)
├── query_executor.rs       # Résolution des requêtes GraphQL
├── mutation_executor.rs    # Résolution des mutations CRUD
├── link_mutations.rs       # Mutations spécifiques aux liens
├── field_resolver.rs       # Résolution des champs et relations
└── utils.rs               # Fonctions utilitaires
```

## 🔧 Composants

### `core.rs` (95 lignes)
**Responsabilité** : Orchestration de l'exécution GraphQL

- `GraphQLExecutor` : Structure principale
- `execute()` : Point d'entrée pour exécuter une query/mutation
- `execute_document()` : Parse et dispatche vers query/mutation
- `execute_query()` : Exécute une opération de requête
- `execute_mutation()` : Exécute une opération de mutation

**Usage** :
```rust
let executor = GraphQLExecutor::new(host).await;
let result = executor.execute(query_string, variables).await?;
```

### `query_executor.rs` (93 lignes)
**Responsabilité** : Résolution des requêtes GraphQL

- `resolve_query_field()` : Résout un champ de requête (`orders`, `order`, etc.)
- `get_entity_type_from_plural()` : Convertit nom pluriel en type d'entité
- `get_entity_type_from_singular()` : Convertit nom singulier en type d'entité

**Exemples de requêtes gérées** :
```graphql
query {
  orders { id, number, customerName }
  order(id: "123") { id, number }
}
```

### `mutation_executor.rs` (165 lignes)
**Responsabilité** : Résolution des mutations CRUD

- `resolve_mutation_field()` : Dispatcher principal pour toutes les mutations
- `create_entity_mutation()` : Crée une nouvelle entité
- `update_entity_mutation()` : Met à jour une entité existante
- `delete_entity_mutation()` : Supprime une entité

**Exemples de mutations gérées** :
```graphql
mutation {
  createOrder(data: { number: "ORD-001", amount: 1000 }) { id }
  updateOrder(id: "123", data: { amount: 1500 }) { id }
  deleteOrder(id: "123")
}
```

### `link_mutations.rs` (240 lignes)
**Responsabilité** : Mutations spécifiques aux liens entre entités

- `create_link_mutation()` : Crée un lien entre deux entités existantes
- `delete_link_mutation()` : Supprime un lien
- `create_and_link_mutation()` : Crée une entité et la lie (`createInvoiceForOrder`)
- `link_entities_mutation()` : Lie deux entités existantes (`linkPaymentToInvoice`)
- `unlink_entities_mutation()` : Délie deux entités (`unlinkPaymentFromInvoice`)

**Exemples de mutations de liens** :
```graphql
mutation {
  # Lien générique
  createLink(sourceId: "order-1", targetId: "invoice-1", linkType: "has_invoice")
  
  # Créer et lier en une seule opération
  createInvoiceForOrder(parentId: "order-1", data: { number: "INV-001" }) { id }
  
  # Lier deux entités existantes
  linkPaymentToInvoice(sourceId: "payment-1", targetId: "invoice-1")
  
  # Délier deux entités
  unlinkPaymentFromInvoice(sourceId: "payment-1", targetId: "invoice-1")
}
```

### `field_resolver.rs` (165 lignes)
**Responsabilité** : Résolution des champs d'entités et des relations

- `resolve_entity_list()` : Résout les champs pour une liste d'entités
- `resolve_entity_fields()` : Résout les champs pour une entité unique
- `resolve_relation_field_impl()` : Résout un champ de relation (e.g., `order.invoices`)

**Gestion des relations** :
- Relations forward : `order.invoices` (1-N)
- Relations reverse : `invoice.order` (N-1)
- Résolution récursive pour requêtes imbriquées
- Utilise `BoxFuture` pour éviter les problèmes de récursion

**Exemple de requête avec relations** :
```graphql
query {
  orders {
    id
    number
    invoices {
      id
      amount
      payments {
        id
        method
      }
    }
  }
}
```

### `utils.rs` (127 lignes)
**Responsabilité** : Fonctions utilitaires partagées

- `get_string_arg()` : Extrait un argument string d'un field GraphQL
- `get_int_arg()` : Extrait un argument int
- `get_json_arg()` : Extrait un argument JSON
- `gql_value_to_json()` : Convertit valeur GraphQL → JSON
- `pluralize()` : Pluralise un mot (`order` → `orders`)
- `pascal_to_snake()` : Convertit PascalCase → snake_case
- `camel_to_snake()` : Convertit camelCase → snake_case
- `mutation_name_to_entity_type()` : Extrait le type d'entité d'un nom de mutation
- `find_link_type()` : Trouve le type de lien dans la configuration

## 🔄 Flux d'Exécution

### Requête GraphQL
```
1. User → GraphQL Query
2. core::execute()
   ↓
3. core::execute_document() → Parse query
   ↓
4. core::execute_query() → Pour chaque field
   ↓
5. query_executor::resolve_query_field()
   ↓ (si liste)
6. EntityFetcher::list_as_json()
   ↓
7. field_resolver::resolve_entity_list()
   ↓ (pour chaque entité)
8. field_resolver::resolve_entity_fields()
   ↓ (si relation)
9. field_resolver::resolve_relation_field()
   ↓
10. LinkService::find_by_source() / find_by_target()
    ↓
11. EntityFetcher::fetch_as_json() pour entités liées
    ↓
12. Récursion pour sous-selections
```

### Mutation GraphQL
```
1. User → GraphQL Mutation
2. core::execute()
   ↓
3. core::execute_document() → Parse mutation
   ↓
4. core::execute_mutation() → Pour chaque field
   ↓
5. mutation_executor::resolve_mutation_field()
   ↓ (dispatch selon type)
6a. mutation_executor::create_entity_mutation()
    ↓
    EntityCreator::create_from_json()
    
6b. link_mutations::create_and_link_mutation()
    ↓
    EntityCreator::create_from_json()
    ↓
    LinkService::create()
    
6c. link_mutations::link_entities_mutation()
    ↓
    LinkService::create()
```

## 🎯 Conventions de Nommage

### Mutations CRUD
- `create{Entity}` → Crée une entité (e.g., `createOrder`)
- `update{Entity}` → Met à jour une entité (e.g., `updateOrder`)
- `delete{Entity}` → Supprime une entité (e.g., `deleteOrder`)

### Mutations de Liens
- `createLink` → Lien générique entre deux entités
- `deleteLink` → Supprime un lien par ID
- `create{Target}For{Source}` → Crée et lie (e.g., `createInvoiceForOrder`)
- `link{Target}To{Source}` → Lie deux entités existantes (e.g., `linkPaymentToInvoice`)
- `unlink{Target}From{Source}` → Délie deux entités (e.g., `unlinkPaymentFromInvoice`)

## 🔧 Gestion des Erreurs

Toutes les fonctions retournent `Result<Value>` pour une gestion d'erreur cohérente :

```rust
// Erreur si argument manquant
if let Some(id) = utils::get_string_arg(field, "id") {
    // ...
} else {
    bail!("Missing required argument 'id'");
}

// Erreur si type d'entité inconnu
if let Some(creator) = host.entity_creators.get(&entity_type) {
    // ...
} else {
    bail!("Unknown entity type: {}", entity_type);
}
```

## 📊 Métriques

| Module               | Lignes | Responsabilité                    |
|---------------------|--------|-----------------------------------|
| `core.rs`           | ~95    | Orchestration                     |
| `query_executor.rs` | ~93    | Résolution de requêtes            |
| `mutation_executor.rs` | ~165 | Mutations CRUD                   |
| `link_mutations.rs` | ~240   | Mutations de liens                |
| `field_resolver.rs` | ~165   | Résolution de champs/relations    |
| `utils.rs`          | ~127   | Utilitaires                       |
| **Total**           | **~885** | **vs 751 avant refactoring**    |

> Note : Le nombre total de lignes a légèrement augmenté (+18%) mais le code est maintenant beaucoup plus modulaire et maintenable.

## 🚀 Avantages de cette Structure

### Avant (executor.rs monolithique)
❌ 751 lignes dans un seul fichier  
❌ Difficile à naviguer  
❌ Couplage élevé entre fonctionnalités  
❌ Tests difficiles à isoler  

### Après (modules séparés)
✅ Modules de ~100-240 lignes chacun  
✅ Responsabilités claires  
✅ Facile à tester unitairement  
✅ Réutilisable (e.g., `utils.rs`)  
✅ Évolutions facilitées  

## 🧪 Testing

Chaque module peut maintenant être testé indépendamment :

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_pluralize() {
        assert_eq!(utils::pluralize("order"), "orders");
        assert_eq!(utils::pluralize("company"), "companies");
    }
    
    #[test]
    fn test_pascal_to_snake() {
        assert_eq!(utils::pascal_to_snake("CreateOrder"), "create_order");
    }
}
```

## 📝 Maintenance

Pour ajouter une nouvelle fonctionnalité :

1. **Nouvelle mutation CRUD** → `mutation_executor.rs`
2. **Nouvelle mutation de lien** → `link_mutations.rs`
3. **Nouveau type de requête** → `query_executor.rs`
4. **Nouvelle logique de résolution** → `field_resolver.rs`
5. **Nouvelle fonction utilitaire** → `utils.rs`

## 🔗 Dépendances

- `graphql-parser` : Parsing des requêtes GraphQL
- `serde_json` : Manipulation de JSON
- `futures` : `BoxFuture` pour récursion async
- `anyhow` : Gestion d'erreurs
- `uuid` : Manipulation d'IDs

## 📚 Références

- [GraphQL Spec](https://spec.graphql.org/)
- [graphql-parser docs](https://docs.rs/graphql-parser/)
- [Async Recursion in Rust](https://rust-lang.github.io/async-book/07_workarounds/04_recursion.html)

