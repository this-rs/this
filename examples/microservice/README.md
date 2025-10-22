# Billing Microservice Example

## Description

Exemple complet d'un microservice de **facturation** (Billing) gérant le workflow Order → Invoice → Payment, démontrant :
- Architecture modulaire propre avec **auto-génération des routes**
- **ServerBuilder** : Zero boilerplate pour le routing
- Navigation bidirectionnelle des liens
- Module system avec trait `Module`
- Store en mémoire (remplaçable par ScyllaDB)
- Authorization policies dans la configuration

## 🚀 La Magie de l'Auto-Génération

Ce microservice utilise le `ServerBuilder` du framework pour **auto-générer toutes les routes** :

```rust
#[tokio::main]
async fn main() -> Result<()> {
    let entity_store = EntityStore::new();
    let module = BillingModule::new(entity_store);

    // ✨ Toutes les routes sont auto-générées ici !
    let app = ServerBuilder::new()
        .with_link_service(InMemoryLinkService::new())
        .register_module(module)?
        .build()?;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
    axum::serve(listener, app).await?;
    Ok(())
}
```

**Zero ligne de routing manuel nécessaire !** Toutes les routes CRUD et de liens sont créées automatiquement.

## Structure

```
microservice/
├── config/              # Configuration externalisée
│   └── links.yaml       # Configuration des entités, liens, et auth
├── store.rs             # Store agrégé (accès aux stores individuels)
├── main.rs              # Point d'entrée (~150 lignes dont 100 de données test)
├── module.rs            # BillingModule (implémente trait Module)
└── entities/            # Un dossier par entité (best practice)
    ├── mod.rs           # Re-exports des entités
    ├── order/
    │   ├── mod.rs       # Module Order
    │   ├── model.rs     # Structure Order
    │   ├── store.rs     # OrderStore (persistance)
    │   ├── handlers.rs  # HTTP handlers Order
    │   └── descriptor.rs # 🆕 EntityDescriptor (auto-registration)
    ├── invoice/
    │   ├── mod.rs
    │   ├── model.rs
    │   ├── store.rs
    │   ├── handlers.rs
    │   └── descriptor.rs # 🆕 EntityDescriptor
    └── payment/
        ├── mod.rs
        ├── model.rs
        ├── store.rs
        ├── handlers.rs
        └── descriptor.rs # 🆕 EntityDescriptor
```

### Fichiers Clés

#### `descriptor.rs` (Nouveau !)

Chaque entité fournit un `EntityDescriptor` qui décrit comment générer ses routes :

```rust
// entities/order/descriptor.rs
pub struct OrderDescriptor {
    pub store: OrderStore,
}

impl EntityDescriptor for OrderDescriptor {
    fn entity_type(&self) -> &str { "order" }
    fn plural(&self) -> &str { "orders" }
    
    fn build_routes(&self) -> Router {
        let state = OrderAppState { store: self.store.clone() };
        Router::new()
            .route("/orders", get(list_orders).post(create_order))
            .route("/orders/:id", get(get_order))
            .with_state(state)
    }
}
```

#### `module.rs`

Le module enregistre tous ses descriptors :

```rust
impl Module for BillingModule {
    fn register_entities(&self, registry: &mut EntityRegistry) {
        registry.register(Box::new(OrderDescriptor::new(self.store.orders.clone())));
        registry.register(Box::new(InvoiceDescriptor::new(self.store.invoices.clone())));
        registry.register(Box::new(PaymentDescriptor::new(self.store.payments.clone())));
    }
}
```

**C'est tout !** Le `ServerBuilder` génère automatiquement toutes les routes.

## Architecture

Cette structure représente l'architecture recommandée pour un vrai microservice :

- **config/** : Configuration externalisée
  - `links.yaml` : Configuration complète (entités, liens, autorisation)
- **entities/** : Dossier contenant toutes les entités
  - **order/** : Tout le code lié aux commandes
    - `model.rs` : Structure Order pure
    - `store.rs` : OrderStore (persistance indépendante)
    - `handlers.rs` : HTTP handlers Order
    - `descriptor.rs` : Auto-registration des routes
  - **invoice/** : Tout le code lié aux factures
  - **payment/** : Tout le code lié aux paiements
- **store.rs** : Store agrégé (accès unifié)
- **module.rs** : BillingModule (trait Module, enregistre les entités)
- **main.rs** : Bootstrap (~50 lignes de code actif, ~100 lignes de données test)

**Séparation claire** : Chaque entité est **complètement isolée** dans son dossier

### Nomenclature Cohérente des Entités

Toutes les entités suivent **exactement la même structure** pour faciliter la compréhension :

```rust
// === Champs communs (TOUTES les entités) ===
id: Uuid              // Identifiant unique
tenant_id: Uuid       // Isolation multi-tenant

// === Champs standards (entités métier) ===
number: String        // Numéro de référence (ORD-001, INV-001, PAY-001)
amount: f64          // Montant
status: String       // Statut (pending/confirmed, draft/sent/paid, pending/completed)

// === Champs spécifiques (propres à chaque entité) ===
// Order: customer_name, notes
// Invoice: due_date, paid_at
// Payment: method, transaction_id
```

**Avantages** :
- ✅ Facile à comprendre : même pattern partout
- ✅ Facile à maintenir : structure cohérente
- ✅ Facile à étendre : ajouter une entité = copier le pattern
- ✅ API prévisible : mêmes champs, mêmes concepts

## Exécution

```bash
cargo run --example microservice
```

Le serveur démarre sur `http://127.0.0.1:3000`

### Output

```
✅ Test data created
🚀 Starting billing-service v1.0.0
📦 Entities: ["order", "invoice", "payment"]

🌐 Server running on http://127.0.0.1:3000

📚 All routes auto-generated:
  - GET    /orders, /invoices, /payments
  - POST   /orders, /invoices, /payments
  - GET    /orders/:id, /invoices/:id, /payments/:id
  - GET    /:entity/:id/:link_route
  - POST   /:entity/:id/:link_type/:target/:target_id
  - DELETE /:entity/:id/:link_type/:target/:target_id
  - GET    /:entity/:id/links
```

## Routes Disponibles (Auto-Générées)

### CRUD Routes (Entités)

Toutes ces routes sont **automatiquement créées** par le `ServerBuilder` :

| Méthode | Route | Description |
|---------|-------|-------------|
| GET | `/orders` | Liste toutes les commandes |
| POST | `/orders` | Crée une nouvelle commande |
| GET | `/orders/{id}` | Récupère une commande spécifique |
| GET | `/invoices` | Liste toutes les factures |
| POST | `/invoices` | Crée une nouvelle facture |
| GET | `/invoices/{id}` | Récupère une facture spécifique |
| GET | `/payments` | Liste tous les paiements |
| POST | `/payments` | Crée un nouveau paiement |
| GET | `/payments/{id}` | Récupère un paiement spécifique |

### Link Routes (Relations)

Ces routes sont également **automatiquement créées** et fonctionnent pour toutes les entités :

| Méthode | Route | Description |
|---------|-------|-------------|
| GET | `/orders/{id}/invoices` | Liste les factures d'une commande |
| GET | `/orders/{id}/invoices/{inv_id}` | Récupère un lien spécifique order→invoice (🆕) |
| GET | `/invoices/{id}/order` | Récupère la commande d'une facture |
| GET | `/invoices/{id}/payments` | Liste les paiements d'une facture |
| GET | `/payments/{id}/invoice` | Récupère la facture d'un paiement |
| POST | `/orders/{id}/invoices/{inv_id}` | Crée un lien order→invoice (🆕 semantic URL) |
| PUT | `/orders/{id}/invoices/{inv_id}` | Met à jour la metadata du lien (🆕) |
| DELETE | `/orders/{id}/invoices/{inv_id}` | Supprime un lien (🆕 semantic URL) |
| GET | `/orders/{id}/links` | Introspection des liens disponibles |

## Exemples de Requêtes

### CRUD Operations

```bash
# Liste toutes les commandes
curl http://127.0.0.1:3000/orders

# Récupère une commande spécifique
curl http://127.0.0.1:3000/orders/<ORDER_ID>

# Crée une nouvelle commande (nomenclature cohérente)
curl -X POST http://127.0.0.1:3000/orders \
  -H "Content-Type: application/json" \
  -d '{
    "number": "ORD-003",
    "amount": 500.0,
    "status": "pending",
    "customer_name": "Charlie Brown",
    "notes": "Urgent delivery"
  }'

# Crée une nouvelle facture (même nomenclature)
curl -X POST http://127.0.0.1:3000/invoices \
  -H "Content-Type: application/json" \
  -d '{
    "number": "INV-004",
    "amount": 250.0,
    "status": "draft",
    "due_date": "2025-12-15"
  }'

# Crée un nouveau paiement (même nomenclature)
curl -X POST http://127.0.0.1:3000/payments \
  -H "Content-Type: application/json" \
  -d '{
    "number": "PAY-003",
    "amount": 250.0,
    "status": "pending",
    "method": "card",
    "transaction_id": "txn_abc123"
  }'

# Liste toutes les factures
curl http://127.0.0.1:3000/invoices

# Liste tous les paiements
curl http://127.0.0.1:3000/payments
```

### Link Navigation

```bash
# Liste les factures d'une commande (avec enrichissement automatique)
curl -H 'X-Tenant-ID: <TENANT_ID>' \
  http://127.0.0.1:3000/orders/<ORDER_ID>/invoices

# Récupère un lien spécifique order→invoice (🆕 avec les deux entités complètes)
curl -H 'X-Tenant-ID: <TENANT_ID>' \
  http://127.0.0.1:3000/orders/<ORDER_ID>/invoices/<INVOICE_ID>

# Récupère la commande d'une facture
curl -H 'X-Tenant-ID: <TENANT_ID>' \
  http://127.0.0.1:3000/invoices/<INVOICE_ID>/order

# Introspection - découvre tous les liens disponibles
curl -H 'X-Tenant-ID: <TENANT_ID>' \
  http://127.0.0.1:3000/orders/<ORDER_ID>/links
```

### Link Manipulation (🆕 Semantic URLs)

```bash
# Crée un lien order → invoice (nouveau format sémantique)
curl -X POST -H 'X-Tenant-ID: <TENANT_ID>' \
  -H 'Content-Type: application/json' \
  -d '{"metadata": {"created_by": "admin", "note": "Initial invoice"}}' \
  http://127.0.0.1:3000/orders/<ORDER_ID>/invoices/<INVOICE_ID>

# Met à jour la metadata d'un lien
curl -X PUT -H 'X-Tenant-ID: <TENANT_ID>' \
  -H 'Content-Type: application/json' \
  -d '{"metadata": {"status": "verified", "verified_by": "manager"}}' \
  http://127.0.0.1:3000/orders/<ORDER_ID>/invoices/<INVOICE_ID>

# Supprime un lien (nouveau format sémantique)
curl -X DELETE -H 'X-Tenant-ID: <TENANT_ID>' \
  http://127.0.0.1:3000/orders/<ORDER_ID>/invoices/<INVOICE_ID>

# Crée un lien invoice → payment
curl -X POST -H 'X-Tenant-ID: <TENANT_ID>' \
  -H 'Content-Type: application/json' \
  -d '{"metadata": {"payment_method": "card", "transaction_id": "txn_123"}}' \
  http://127.0.0.1:3000/invoices/<INVOICE_ID>/payments/<PAYMENT_ID>
```

**Note** : Le nouveau format utilise `route_name` au lieu de `link_type` pour des URLs plus sémantiques :
- ✅ `/orders/{id}/invoices/{invoice_id}` (semantic, auto-documenté)
- ❌ `/orders/{id}/has_invoice/invoices/{invoice_id}` (ancien format, plus verbeux)

## Ce Que Vous Apprendrez

### Architecture
- ✅ Structure modulaire propre et maintenable
- ✅ **ServerBuilder** : Auto-génération des routes
- ✅ **EntityDescriptor** : Pattern pour déclarer les routes
- ✅ Séparation des responsabilités (entities/store/handlers/descriptor/module)
- ✅ Pattern Repository avec `EntityStore`

### Framework Features
- ✅ Trait `Module` pour définir un microservice
- ✅ **Auto-registration** des entités via `register_entities()`
- ✅ Configuration YAML avec auth policies
- ✅ Routes CRUD **auto-générées** (zero boilerplate)
- ✅ Routes de liens **auto-générées** (génériques)
- ✅ Navigation bidirectionnelle des liens
- ✅ Store en mémoire (pattern pour ScyllaDB)

### Production Ready
- ✅ Multi-tenant support (tenant_id)
- ✅ Authorization policies déclaratives
- ✅ Structure prête pour ScyllaDB
- ✅ Code organization professionnelle
- ✅ **Zero boilerplate** dans main.rs

## Ajouter une Nouvelle Entité

Grâce à l'auto-génération, ajouter une entité est trivial :

### 1. Créer l'entité

```rust
// entities/product/model.rs
pub struct Product {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub number: String,
    pub amount: f64,
    pub status: String,
    pub name: String,
}
```

### 2. Créer le store

```rust
// entities/product/store.rs
pub struct ProductStore { /* ... */ }
```

### 3. Créer les handlers

```rust
// entities/product/handlers.rs
pub async fn list_products(...) { /* ... */ }
pub async fn get_product(...) { /* ... */ }
pub async fn create_product(...) { /* ... */ }
```

### 4. Créer le descriptor

```rust
// entities/product/descriptor.rs
pub struct ProductDescriptor {
    pub store: ProductStore,
}

impl EntityDescriptor for ProductDescriptor {
    fn entity_type(&self) -> &str { "product" }
    fn plural(&self) -> &str { "products" }
    
    fn build_routes(&self) -> Router {
        let state = ProductAppState { store: self.store.clone() };
        Router::new()
            .route("/products", get(list_products).post(create_product))
            .route("/products/:id", get(get_product))
            .with_state(state)
    }
}
```

### 5. Enregistrer dans le module

```rust
// module.rs
impl Module for BillingModule {
    fn register_entities(&self, registry: &mut EntityRegistry) {
        registry.register(Box::new(OrderDescriptor::new(...)));
        registry.register(Box::new(InvoiceDescriptor::new(...)));
        registry.register(Box::new(PaymentDescriptor::new(...)));
        registry.register(Box::new(ProductDescriptor::new(...))); // ← Ajouter ici
    }
}
```

### 6. Ajouter dans config/links.yaml

```yaml
entities:
  - singular: product
    plural: products
    auth:
      list: authenticated
      get: authenticated
      create: authenticated
```

**C'est tout !** Les routes `/products`, `/products/:id` sont automatiquement créées.

**Aucune modification de `main.rs` nécessaire !**

## Migration vers Production

Pour utiliser en production, remplacez `EntityStore` par une implémentation ScyllaDB :

```rust
// store.rs
use scylla::Session;

pub struct ScyllaEntityStore {
    session: Arc<Session>,
}

impl ScyllaEntityStore {
    pub async fn get_order(&self, id: &Uuid) -> Result<Option<Order>> {
        let query = "SELECT * FROM orders WHERE id = ?";
        // ... ScyllaDB query
    }
}
```

## Avantages de l'Auto-Génération

### Avant (Approche Manuelle)

```rust
// main.rs - 340 lignes de boilerplate
let app = Router::new()
    .route("/orders", get(list_orders).post(create_order))
    .route("/orders/:id", get(get_order))
    .with_state(order_state)
    .route("/invoices", get(list_invoices).post(create_invoice))
    .route("/invoices/:id", get(get_invoice))
    .with_state(invoice_state)
    // ... 30+ lignes par entité
```

### Après (Avec ServerBuilder)

```rust
// main.rs - ~40 lignes de code actif
let app = ServerBuilder::new()
    .with_link_service(InMemoryLinkService::new())
    .register_module(module)?  // ← Tout se passe ici !
    .build()?;
```

**Réduction : -88% de code !**

### Bénéfices

✅ **Zero boilerplate** : Aucune déclaration manuelle de routes  
✅ **Consistance garantie** : Toutes les entités ont les mêmes routes  
✅ **Scalabilité infinie** : 3 ou 300 entités = même simplicité  
✅ **Maintenabilité** : Modifier le pattern une fois pour toutes  
✅ **Type-safe** : Vérification complète à la compilation  
✅ **Lisibilité** : Le code exprime l'intention, pas les détails  

## Prochaines Étapes

1. Implémenter `ScyllaDBLinkService` et `ScyllaEntityStore`
2. Ajouter `JwtAuthProvider` pour authentication
3. Intégrer les auth policies dans les handlers
4. Ajouter pagination, filtres, tri
5. Ajouter UPDATE/DELETE pour les entités
6. Monitoring et métriques (Prometheus)
7. Healthchecks et graceful shutdown

Tout est documenté dans :
- `SERVER_BUILDER_IMPLEMENTATION.md` - Architecture détaillée
- `AUTO_ROUTING_SUCCESS.md` - Résumé de l'implémentation
- `ROUTING_EXPLANATION.md` - Explications architecturales

---

**Ce microservice démontre la puissance du framework This-RS : déclarez vos entités, et laissez le framework gérer le reste !** 🚀🦀✨
