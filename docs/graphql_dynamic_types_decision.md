# Décision: Types GraphQL Dynamiques vs Génériques

## Situation Actuelle

✅ **Ce qui fonctionne:**
- Schéma SDL dynamique généré automatiquement (`/graphql/schema`)
- Types spécifiques dans le schéma (`Order`, `Invoice`, `Payment`) avec tous leurs champs
- Queries et mutations CRUD fonctionnelles
- Navigation des liens entre entités

❌ **Le problème:**
- Les resolvers retournent `JsonValue` (JSON générique)
- Les requêtes GraphQL ne peuvent pas accéder aux champs de manière typée
- Impossible de faire `orders { id number customerName }` - il faut faire `entities(entityType: "order")`

## Les 3 Options

### Option 1: Rester avec le Système Actuel (JSON Générique)

**Schéma actuel:**
```graphql
type Query {
  entityTypes: [String!]!
  entity(id: ID!, entityType: String!): JsonValue
  entities(entityType: String!, limit: Int, offset: Int): [JsonValue!]!
}
```

**Exemple de requête:**
```graphql
query {
  entities(entityType: "order", limit: 10)
}
```

**Résultat:**
```json
{
  "data": {
    "entities": [
      {
        "id": "...",
        "number": "ORD-001",
        "customerName": "Alice",
        "amount": 999.99,
        ...tous les champs...
      }
    ]
  }
}
```

✅ **Avantages:**
- 100% générique - fonctionne avec n'importe quelle entité
- Aucun code supplémentaire pour le développeur
- Déjà implémenté et fonctionnel

❌ **Inconvénients:**
- Pas de typage dans les requêtes GraphQL
- Pas d'autocomplétion dans GraphiQL
- Schéma moins idiomatique

---

### Option 2: Macro `graphql_entity!` (Code Supplémentaire)

Le développeur ajoute une ligne à chaque entité :

```rust
impl_data_entity_validated!(Order, "order", ..., { ... });

// Nouvelle ligne à ajouter:
graphql_entity!(Order, relations: { invoices: [Invoice] });
```

**Schéma généré:**
```graphql
type Order {
  id: ID!
  number: String!
  customerName: String!
  amount: Float!
  invoices: [Invoice!]!
}

type Query {
  order(id: ID!): Order
  orders(limit: Int): [Order!]!
}
```

**Exemple de requête:**
```graphql
query {
  orders(limit: 10) {
    id
    number
    customerName
    invoices {
      id
      amount
    }
  }
}
```

✅ **Avantages:**
- Types spécifiques et typés
- Navigation de relations élégante
- Autocomplétion dans GraphiQL
- Schéma idiomatique

❌ **Inconvénients:**
- Code déclaratif supplémentaire (une ligne par entité)
- Plus complexe à implémenter (estimation: 2-3h)
- Nécessite de dupliquer les définitions de champs

---

### Option 3: Génération Automatique via build.rs (Complexe)

Le framework scan les entités à la compilation et génère automatiquement les types GraphQL.

✅ **Avantages:**
- Types spécifiques
- AUCUN code supplémentaire
- Vraiment "invisible" pour le développeur

❌ **Inconvénients:**
- Très complexe à implémenter (estimation: 1-2 jours)
- Temps de compilation plus long
- Difficile à déboguer
- Peut avoir des problèmes avec les IDE

---

## Recommandation

### Pour un prototype / MVP: **Option 1** (actuel)
- Fonctionne maintenant
- Permet de tester le framework
- Peut être amélioré plus tard

### Pour la production: **Option 2** (macro)
- Bon compromis complexité/bénéfices
- Une seule ligne par entité
- Types GraphQL propres

### Pour l'idéal (long terme): **Option 3** (build.rs)
- Vraiment zéro configuration
- Mais complexe et risqué

## Exemple Comparatif

### Avec Option 1 (Actuel - JSON):
```graphql
query {
  entities(entityType: "order") {
    # Retourne JSON brut
  }
}
```

### Avec Option 2/3 (Types Spécifiques):
```graphql
query {
  orders {
    id
    number
    customerName
    amount
    invoices {
      id
      payments {
        id
        amount
      }
    }
  }
}
```

## Décision?

Quelle option préfères-tu? Je peux implémenter l'Option 2 (macro) maintenant si tu veux.

