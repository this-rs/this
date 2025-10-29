# Exemple Microservice avec GraphQL

Cet exemple démontre l'utilisation de l'exposition GraphQL du framework `this-rs` aux côtés de l'API REST.

## 🎯 Fonctionnalités

- ✅ **Queries nommées par type d'entité** : `order(id)`, `invoice(id)`, `payment(id)`
- ✅ **Relations automatiques** : Accès aux entités liées via des champs (`order.invoices`, `invoice.payments`)
- ✅ **Relations imbriquées** : Navigation profonde (`order.invoices.payments`)
- ✅ **Schéma généré automatiquement** : Basé sur les entités enregistrées par les modules
- ✅ **Playground GraphQL** : Interface interactive pour tester les requêtes

## Démarrage

```bash
cargo run --example microservice_graphql --features graphql
```

Le serveur démarre sur `http://127.0.0.1:3000` avec :

## Tests automatiques

Un script de test est fourni pour valider toutes les fonctionnalités GraphQL :

```bash
# Démarrer le serveur dans un terminal
cargo run --example microservice_graphql --features graphql

# Dans un autre terminal, lancer les tests
cd examples/microservice
./test_graphql.sh
```

Le script teste :
- ✅ Récupération du schéma SDL
- ✅ Liste des types d'entités
- ✅ Queries par nom d'entité
- ✅ Relations simples (order->invoices)
- ✅ Relations imbriquées (order->invoices->payments)
- ✅ Mutations (création de liens)

## Endpoints disponibles
- **API REST** : Tous les endpoints CRUD standards
- **API GraphQL** : 
  - Endpoint `/graphql` (POST)
  - Playground `/graphql/playground` (GET)
  - Schema SDL `/graphql/schema` (GET)

## GraphQL Playground

Accédez au playground interactif : http://127.0.0.1:3000/graphql/playground

## GraphQL Schema

**Téléchargez le schéma SDL :** http://127.0.0.1:3000/graphql/schema

Le schéma est **généré automatiquement** à partir des entités enregistrées :
- ✅ Types GraphQL spécifiques pour chaque entité (`Order`, `Invoice`, `Payment`)
- ✅ Tous les champs découverts automatiquement depuis les données
- ✅ Relations automatiques depuis `links.yaml`
- ✅ Queries et mutations CRUD complètes
- ✅ **100% générique** - aucun code hardcodé dans le framework

Le schéma SDL (Schema Definition Language) est utile pour :
- Générer des clients GraphQL typés
- Documentation automatique
- Validation des requêtes
- Intégration avec des outils comme GraphQL Code Generator

## Requêtes GraphQL disponibles

### ⭐ Nouveauté : Queries par nom d'entité

Au lieu d'utiliser `entity(id, entityType)`, vous pouvez maintenant interroger directement par type :

```graphql
query {
  order(id: "UUID") {
    id
    name
    status
    data
  }
}
```

### 🔗 Relations automatiques

Les entités exposent automatiquement leurs relations via des champs :

```graphql
query {
  order(id: "UUID") {
    id
    name
    invoices {
      id
      name
      payments {
        id
        name
      }
    }
  }
}
```

**Résultat** :
```json
{
  "data": {
    "order": {
      "id": "d16e72cf-d7f7-41f4-aa86-ca428967fa0a",
      "name": "ORD-001",
      "invoices": [
        {
          "id": "b5ef6156-0dcb-49fd-b425-5805044ddbc4",
          "name": "INV-002",
          "payments": [
            {
              "id": "90164a77-d517-4c27-8677-ac56a665cb9c",
              "name": "PAY-002"
            }
          ]
        }
      ]
    }
  }
}
```

### Lister les types d'entités

```graphql
query {
  entityTypes
}
```

**Résultat** :
```json
{
  "data": {
    "entityTypes": ["order", "invoice", "payment"]
  }
}
```

### Récupérer une entité par ID (méthode générique)

```graphql
query {
  entity(id: "UUID", entityType: "order") {
    id
    type
    name
    status
    createdAt
    updatedAt
    data
  }
}
```

### Récupérer les liens d'une entité

```graphql
query {
  entityLinks(entityId: "UUID") {
    id
    linkType
    sourceId
    targetId
    metadata
    createdAt
  }
}
```

**Avec filtres** :
```graphql
query {
  entityLinks(
    entityId: "UUID"
    linkType: "invoices"
    targetType: "invoice"
  ) {
    id
    linkType
    targetId
    metadata
  }
}
```

### Récupérer un lien spécifique

```graphql
query {
  link(id: "UUID") {
    id
    linkType
    sourceId
    targetId
    metadata
    createdAt
  }
}
```

## Mutations GraphQL disponibles

### Créer un lien

```graphql
mutation {
  createLink(
    sourceId: "UUID"
    targetId: "UUID"
    linkType: "invoices"
    metadata: {note: "Test link", priority: "high"}
  ) {
    id
    linkType
    sourceId
    targetId
    metadata
    createdAt
  }
}
```

**Sans metadata** :
```graphql
mutation {
  createLink(
    sourceId: "UUID"
    targetId: "UUID"
    linkType: "invoices"
  ) {
    id
    linkType
  }
}
```

### Supprimer un lien

```graphql
mutation {
  deleteLink(id: "UUID")
}
```

**Résultat** : `true` si supprimé, `false` sinon.

## Exemples avec curl

### Récupérer le schéma SDL

```bash
curl http://127.0.0.1:3000/graphql/schema
```

Cela retourne le schéma complet au format SDL, incluant tous les types, queries, et mutations disponibles.

### Lister les types

```bash
curl -X POST http://127.0.0.1:3000/graphql \
  -H "Content-Type: application/json" \
  -d '{"query": "query { entityTypes }"}'
```

### Récupérer un order avec ses relations

```bash
# Récupérer un ID depuis REST
ORDER_ID=$(curl -s http://127.0.0.1:3000/orders | jq -r '.data[0].id')

# Query GraphQL avec relations
curl -X POST http://127.0.0.1:3000/graphql \
  -H "Content-Type: application/json" \
  -d "{\"query\": \"query { order(id: \\\"$ORDER_ID\\\") { id name invoices { id name payments { id name } } } }\"}"
```

### Récupérer une invoice avec ses payments

```bash
INVOICE_ID=$(curl -s http://127.0.0.1:3000/invoices | jq -r '.data[0].id')

curl -X POST http://127.0.0.1:3000/graphql \
  -H "Content-Type: application/json" \
  -d "{\"query\": \"query { invoice(id: \\\"$INVOICE_ID\\\") { id name payments { id name } } }\"}"
```

### Récupérer les liens (méthode générique)

```bash
ORDER_ID=$(curl -s http://127.0.0.1:3000/orders | jq -r '.data[0].id')

curl -X POST http://127.0.0.1:3000/graphql \
  -H "Content-Type: application/json" \
  -d "{\"query\": \"query { entityLinks(entityId: \\\"$ORDER_ID\\\") { id linkType targetId metadata } }\"}"
```

### Créer un lien

```bash
ORDER_ID=$(curl -s http://127.0.0.1:3000/orders | jq -r '.data[0].id')
INVOICE_ID=$(curl -s http://127.0.0.1:3000/invoices | jq -r '.data[0].id')

curl -X POST http://127.0.0.1:3000/graphql \
  -H "Content-Type: application/json" \
  -d "{\"query\": \"mutation { createLink(sourceId: \\\"$ORDER_ID\\\", targetId: \\\"$INVOICE_ID\\\", linkType: \\\"test_link\\\") { id linkType } }\"}"
```

## Architecture

L'exemple combine deux expositions du framework :

1. **REST Exposure** (`RestExposure`) : Fournit l'API REST classique
2. **GraphQL Exposure** (`GraphQLExposure`) : Fournit l'API GraphQL

Les deux expositions partagent le même `ServerHost`, qui contient :
- Configuration des liens
- Service de liens
- Registre des entités
- Fetchers et creators d'entités

## Notes techniques

- Le schema GraphQL est généré automatiquement à partir des entités enregistrées
- Les types GraphQL sont génériques (`Entity`, `Link`) pour supporter toutes les entités
- Le champ `data` de l'entité contient tous les champs custom en JSON
- Les metadata des liens sont aussi en JSON pour flexibilité maximale
- Le playground GraphQL est disponible uniquement en mode développement

## ✅ Fonctionnalités implémentées

- ✅ Queries nommées par type d'entité (`order`, `invoice`, `payment`)
- ✅ Relations automatiques entre entités
- ✅ Navigation imbriquée (relations de relations)
- ✅ Mutations pour créer/supprimer des liens
- ✅ Query générique `entity(id, entityType)` pour flexibilité
- ✅ GraphQL Playground pour tests interactifs
- ✅ Export du schéma SDL via `/graphql/schema`

## Limitations actuelles

- ⚠️ Les queries `orders`, `invoices`, `payments` (listes) ne sont pas encore implémentées
- ⚠️ Pas de pagination GraphQL (à venir)
- ⚠️ Pas de filtres GraphQL sur les entités (à venir)
- ⚠️ Pas de création/mise à jour d'entités via GraphQL (à venir)
- ⚠️ Subscriptions GraphQL non implémentées
- ⚠️ Les types d'entités sont hardcodés (`order`, `invoice`, `payment`) - à générer dynamiquement

## Prochaines étapes

- [ ] Générer dynamiquement les queries pour tous les types d'entités enregistrés
- [ ] Ajouter la pagination dans les requêtes GraphQL
- [ ] Ajouter les mutations CRUD pour les entités
- [ ] Ajouter les filtres et le tri
- [ ] Ajouter les subscriptions pour les changements en temps réel

