# Billing Microservice Example

## Description

Exemple complet d'un microservice de **facturation** (Billing) gérant le workflow Order → Invoice → Payment, démontrant :
- Architecture modulaire propre
- **Routes CRUD génériques** (zero boilerplate)
- Navigation bidirectionnelle des liens
- Module system avec trait `Module`
- Store en mémoire (remplaçable par ScyllaDB)
- Authorization policies dans la configuration

## Structure

```
microservice/
├── config/              # Configuration externalisée
│   └── links.yaml       # Configuration des entités, liens, et auth
├── crud_handlers.rs     # 🆕 Handlers CRUD génériques (zero boilerplate)
├── store.rs             # 🆕 Store agrégé (accès unifié)
├── main.rs              # Point d'entrée et setup du serveur
├── module.rs            # Module trait (BillingModule)
└── entities/            # Un dossier par entité (best practice)
    ├── mod.rs           # Re-exports des entités
    ├── order/
    │   ├── mod.rs       # Module Order
    │   ├── model.rs     # Structure Order
    │   ├── store.rs     # OrderStore (persistance)
    │   └── handlers.rs  # Handlers HTTP Order
    ├── invoice/
    │   ├── mod.rs       # Module Invoice
    │   ├── model.rs     # Structure Invoice
    │   ├── store.rs     # InvoiceStore (persistance)
    │   └── handlers.rs  # Handlers HTTP Invoice
    └── payment/
        ├── mod.rs       # Module Payment
        ├── model.rs     # Structure Payment
        ├── store.rs     # PaymentStore (persistance)
        └── handlers.rs  # Handlers HTTP Payment
```

**Best Practice** : Chaque entité a son propre dossier avec :
- `model.rs` : Structure de données pure
- `store.rs` : Couche de persistance
- `handlers.rs` : Couche HTTP/API
- `mod.rs` : Exports du module

## Architecture

Cette structure représente l'architecture recommandée pour un vrai microservice :

- **config/** : Configuration externalisée
  - `links.yaml` : Configuration complète (entités, liens, autorisation)
- **entities/** : Dossier contenant toutes les entités
  - **order/** : Tout le code lié aux commandes
    - `model.rs` : Structure Order pure
    - `store.rs` : OrderStore (persistance indépendante)
    - `handlers.rs` : HTTP handlers Order
  - **invoice/** : Tout le code lié aux factures
    - `store.rs` : InvoiceStore (persistance indépendante)
  - **payment/** : Tout le code lié aux paiements
    - `store.rs` : PaymentStore (persistance indépendante)
- **module.rs** : BillingModule (trait Module, charge config/links.yaml)
- **main.rs** : Bootstrap et wiring (utilise directement les stores individuels)

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

## Routes Disponibles

### CRUD Routes (Entités)

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

| Méthode | Route | Description |
|---------|-------|-------------|
| GET | `/orders/{id}/invoices` | Liste les factures d'une commande |
| GET | `/invoices/{id}/order` | Récupère la commande d'une facture |
| GET | `/invoices/{id}/payments` | Liste les paiements d'une facture |
| GET | `/payments/{id}/invoice` | Récupère la facture d'un paiement |
| POST | `/orders/{id}/has_invoice/invoices/{inv_id}` | Crée un lien |
| DELETE | `/orders/{id}/has_invoice/invoices/{inv_id}` | Supprime un lien |
| GET | `/orders/{id}/links` | Introspection |

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
# Liste les factures d'une commande
curl -H 'X-Tenant-ID: <TENANT_ID>' \
  http://127.0.0.1:3000/orders/<ORDER_ID>/invoices

# Récupère la commande d'une facture
curl -H 'X-Tenant-ID: <TENANT_ID>' \
  http://127.0.0.1:3000/invoices/<INVOICE_ID>/order

# Introspection - découvre tous les liens disponibles
curl -H 'X-Tenant-ID: <TENANT_ID>' \
  http://127.0.0.1:3000/orders/<ORDER_ID>/links
```

## Ce Que Vous Apprendrez

### Architecture
- ✅ Structure modulaire propre et maintenable
- ✅ Séparation des responsabilités (entities/store/handlers/module)
- ✅ Pattern Repository avec `EntityStore`

### Framework Features
- ✅ Trait `Module` pour définir un microservice
- ✅ Configuration YAML avec auth policies
- ✅ Routes CRUD auto-générées
- ✅ Navigation bidirectionnelle des liens
- ✅ Store en mémoire (pattern pour ScyllaDB)

### Production Ready
- ✅ Multi-tenant support (tenant_id)
- ✅ Authorization policies déclaratives
- ✅ Structure prête pour ScyllaDB
- ✅ Code organisation professionnelle

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

Voir `ARCHITECTURE_MICROSERVICES.md` pour le guide complet.

## Prochaines Étapes

1. Implémenter `ScyllaDBLinkService` et `ScyllaEntityStore`
2. Ajouter `JwtAuthProvider` pour authentication
3. Intégrer les auth policies dans les handlers
4. Ajouter pagination, filtres, tri
5. Ajouter UPDATE/DELETE pour les entités
6. Monitoring et métriques (Prometheus)
7. Healthchecks et graceful shutdown

Tout est documenté dans :
- `ARCHITECTURE_MICROSERVICES.md`
- `IMPLEMENTATION_COMPLETE.md`
- `START_HERE.md`

