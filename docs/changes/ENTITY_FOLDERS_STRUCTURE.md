# ✅ Structure par Dossiers d'Entités - Best Practice

## 🎯 Objectif

Restructurer le code pour que **chaque entité ait son propre dossier** contenant tout son code (model, store, handlers). C'est une **best practice** pour les microservices en production.

## 📊 Avant / Après

### Avant (Fichiers centralisés)

```
microservice/
├── main.rs          # Bootstrap
├── entities.rs      # ❌ TOUTES les entités (56 lignes)
├── store.rs         # Store centralisé
├── handlers.rs      # ❌ TOUS les handlers (137 lignes)
└── module.rs        # Configuration
```

**Problèmes** :
- ❌ Fichiers monolithiques qui grandissent indéfiniment
- ❌ Tout le code d'une entité éparpillé dans 3 fichiers
- ❌ Difficile de trouver le code spécifique à Order
- ❌ Pas de séparation claire des responsabilités
- ❌ Conflits de merge fréquents (tout le monde édite `handlers.rs`)

### Après (Structure modulaire par entité)

```
microservice/
├── main.rs
├── module.rs
├── store.rs
└── entities/
    ├── mod.rs           # Re-exports
    ├── order/           # ✅ Tout Order dans un dossier
    │   ├── mod.rs       # Exports Order
    │   ├── model.rs     # Structure Order (20 lignes)
    │   ├── store.rs     # OrderStore (35 lignes)
    │   └── handlers.rs  # Order handlers (55 lignes)
    ├── invoice/         # ✅ Tout Invoice dans un dossier
    │   ├── mod.rs
    │   ├── model.rs     # Structure Invoice (18 lignes)
    │   ├── store.rs     # InvoiceStore (35 lignes)
    │   └── handlers.rs  # Invoice handlers (55 lignes)
    └── payment/         # ✅ Tout Payment dans un dossier
        ├── mod.rs
        ├── model.rs     # Structure Payment (19 lignes)
        ├── store.rs     # PaymentStore (35 lignes)
        └── handlers.rs  # Payment handlers (57 lignes)
```

**Avantages** :
- ✅ Code d'une entité 100% isolé dans son dossier
- ✅ Facile à naviguer : tout Order est dans `entities/order/`
- ✅ Séparation claire : model/store/handlers
- ✅ Scalable : ajouter une entité = ajouter un dossier
- ✅ Pas de conflits de merge (chacun travaille dans son dossier)
- ✅ Architecture production-ready

## 🗂️ Structure Détaillée

### entities/order/

```
order/
├── mod.rs           # Module exports
│   pub mod handlers;
│   pub mod model;
│   pub mod store;
│   pub use model::Order;
│   pub use handlers::*;
│   pub use store::OrderStore;
│
├── model.rs         # Structure de données PURE
│   #[derive(Debug, Clone, Serialize, Deserialize)]
│   pub struct Order {
│       pub id: Uuid,
│       pub tenant_id: Uuid,
│       pub number: String,
│       pub amount: f64,
│       pub status: String,
│       pub customer_name: Option<String>,
│       pub notes: Option<String>,
│   }
│
├── store.rs         # Persistance
│   pub struct OrderStore {
│       data: Arc<RwLock<HashMap<Uuid, Order>>>,
│   }
│   impl OrderStore {
│       pub fn new() -> Self { ... }
│       pub fn add(&self, order: Order) { ... }
│       pub fn get(&self, id: &Uuid) -> Option<Order> { ... }
│       pub fn list(&self) -> Vec<Order> { ... }
│   }
│
└── handlers.rs      # HTTP handlers
    pub struct OrderAppState { ... }
    pub async fn list_orders(...) -> Json<Value> { ... }
    pub async fn get_order(...) -> Result<Json<Order>, StatusCode> { ... }
    pub async fn create_order(...) -> Result<Json<Order>, StatusCode> { ... }
```

**Pattern reproduit** pour `invoice/` et `payment/` !

## 🔧 Changements Techniques

### 1. Création des Dossiers

```bash
mkdir -p examples/microservice/entities/{order,invoice,payment}
```

### 2. Fichiers Créés (13 fichiers)

| Fichier | Lignes | Rôle |
|---------|--------|------|
| **entities/mod.rs** | 8 | Re-exports globaux |
| **order/mod.rs** | 8 | Exports Order |
| **order/model.rs** | 20 | Structure Order |
| **order/store.rs** | 35 | OrderStore |
| **order/handlers.rs** | 55 | HTTP handlers Order |
| **invoice/mod.rs** | 8 | Exports Invoice |
| **invoice/model.rs** | 18 | Structure Invoice |
| **invoice/store.rs** | 35 | InvoiceStore |
| **invoice/handlers.rs** | 55 | HTTP handlers Invoice |
| **payment/mod.rs** | 8 | Exports Payment |
| **payment/model.rs** | 19 | Structure Payment |
| **payment/store.rs** | 35 | PaymentStore |
| **payment/handlers.rs** | 57 | HTTP handlers Payment |

**Total** : 13 fichiers, ~360 lignes (vs 2 fichiers, ~193 lignes avant)

### 3. Fichiers Supprimés

- ❌ `examples/microservice/entities.rs` (56 lignes monolithique)
- ❌ `examples/microservice/handlers.rs` (137 lignes monolithique)

### 4. Fichiers Modifiés

**main.rs** :
```rust
// Avant
mod entities;
mod handlers;
use entities::{Invoice, Order, Payment};
use handlers::*;

// Après
mod entities;
use entities::{
    invoice::{create_invoice, get_invoice, list_invoices, InvoiceAppState},
    order::{create_order, get_order, list_orders, OrderAppState},
    payment::{create_payment, get_payment, list_payments, PaymentAppState},
    Invoice, Order, Payment,
};
```

## ✨ Bénéfices

### 1. Organisation Claire

```
❓ Où est le code Order ?
✅ Tout dans entities/order/

❓ Où est le handler create_order ?
✅ entities/order/handlers.rs

❓ Où est la structure Order ?
✅ entities/order/model.rs
```

### 2. Scalabilité

Ajouter une nouvelle entité = copier le dossier :

```bash
cp -r entities/order entities/product
# Renommer Order → Product
# C'est tout !
```

### 3. Parallélisation du Développement

```
👨‍💻 Dev A travaille sur entities/order/
👩‍💻 Dev B travaille sur entities/invoice/
👨‍💻 Dev C travaille sur entities/payment/

→ Zéro conflit de merge !
```

### 4. Tests Isolés

```rust
// tests/order_tests.rs
use crate::entities::order::*;
// Tests uniquement pour Order

// tests/invoice_tests.rs
use crate::entities::invoice::*;
// Tests uniquement pour Invoice
```

### 5. Ownership d'Équipe

```
Team Orders possède entities/order/
Team Billing possède entities/invoice/ et entities/payment/

→ Responsabilités claires !
```

## 🎓 Pattern Recommandé

Pour **chaque nouvelle entité**, créez cette structure :

```
entities/<entity_name>/
├── mod.rs           # Exports
├── model.rs         # Structure + derives
├── store.rs         # Persistance
└── handlers.rs      # HTTP handlers
```

### Template mod.rs

```rust
//! <Entity> entity module

pub mod handlers;
pub mod model;
pub mod store;

pub use handlers::*;
pub use model::<Entity>;
pub use store::<Entity>Store;
```

### Template model.rs

```rust
//! <Entity> entity model

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct <Entity> {
    // === Common fields ===
    pub id: Uuid,
    pub tenant_id: Uuid,
    
    // === Standard fields ===
    pub number: String,
    pub amount: f64,
    pub status: String,
    
    // === Specific fields ===
    pub custom_field: String,
}
```

### Template store.rs

```rust
//! <Entity> store implementation

use super::model::<Entity>;
use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use uuid::Uuid;

#[derive(Clone)]
pub struct <Entity>Store {
    data: Arc<RwLock<HashMap<Uuid, <Entity>>>>,
}

impl <Entity>Store {
    pub fn new() -> Self { ... }
    pub fn add(&self, item: <Entity>) { ... }
    pub fn get(&self, id: &Uuid) -> Option<<Entity>> { ... }
    pub fn list(&self) -> Vec<<Entity>> { ... }
}

impl Default for <Entity>Store {
    fn default() -> Self { Self::new() }
}
```

### Template handlers.rs

```rust
//! <Entity> HTTP handlers

use super::model::<Entity>;
use crate::store::EntityStore;
use axum::{extract::{Path, State}, http::StatusCode, response::Json};
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Clone)]
pub struct <Entity>AppState {
    pub entity_store: EntityStore,
}

pub async fn list_<entity_plural>(State(state): State<<Entity>AppState>) -> Json<Value> {
    let items = state.entity_store.list_<entity_plural>();
    Json(json!({
        "<entity_plural>": items,
        "count": items.len()
    }))
}

pub async fn get_<entity>(
    State(state): State<<Entity>AppState>,
    Path(id): Path<String>,
) -> Result<Json<<Entity>>, StatusCode> {
    let id = Uuid::parse_str(&id).map_err(|_| StatusCode::BAD_REQUEST)?;
    state.entity_store.get_<entity>(&id).map(Json).ok_or(StatusCode::NOT_FOUND)
}

pub async fn create_<entity>(
    State(state): State<<Entity>AppState>,
    Json(payload): Json<Value>,
) -> Result<Json<<Entity>>, StatusCode> {
    // Implementation
}
```

## 🚀 Migration Guide

### Étape 1 : Créer les Dossiers

```bash
mkdir -p entities/{order,invoice,payment}
```

### Étape 2 : Créer les Fichiers

Pour chaque entité, créer `mod.rs`, `model.rs`, `store.rs`, `handlers.rs`

### Étape 3 : Déplacer le Code

```bash
# Extraire Order de entities.rs → order/model.rs
# Extraire OrderStore de store.rs → order/store.rs
# Extraire handlers Order de handlers.rs → order/handlers.rs
```

### Étape 4 : Mettre à Jour les Imports

```rust
// main.rs
use entities::{
    order::{list_orders, get_order, create_order, OrderAppState},
    Order,
};
```

### Étape 5 : Supprimer les Anciens Fichiers

```bash
rm entities.rs handlers.rs
```

### Étape 6 : Tester

```bash
cargo build --example microservice
cargo run --example microservice
```

## 📊 Comparaison

| Critère | Avant (Centralisé) | Après (Par Dossier) |
|---------|-------------------|-------------------|
| **Fichiers par entité** | 0 (code mélangé) | 4 (isolés) |
| **Lignes max/fichier** | 137 lignes | ~60 lignes |
| **Navigation** | ❌ Difficile | ✅ Intuitive |
| **Scalabilité** | ❌ Limitée | ✅ Infinie |
| **Conflits merge** | ❌ Fréquents | ✅ Rares |
| **Ownership** | ❌ Flou | ✅ Clair |
| **Tests isolés** | ❌ Difficile | ✅ Facile |
| **Production-ready** | ❌ Non | ✅ Oui |

## 🎯 Résultat

### Avant
```
Code éparpillé dans 3 fichiers centraux
Difficile à naviguer
Pas scalable
```

### Après
```
Code isolé par entité
Navigation intuitive
Scalable à l'infini
Production-ready
```

## ✅ Tests de Validation

```bash
# ✅ Structure claire
tree entities/
# → 3 dossiers, 13 fichiers

# ✅ Compilation
cargo build --example microservice
# → Success

# ✅ Code isolé
cat entities/order/model.rs
# → Uniquement Order, rien d'autre
```

## 🎓 Apprentissage

Cette structure enseigne :

1. **Modularité** : Chaque entité est un module complet
2. **Séparation** : model/store/handlers clairement séparés
3. **Scalabilité** : Pattern reproductible à l'infini
4. **Best practices** : Architecture production-ready
5. **Organisation** : Code facile à trouver et maintenir

## 🌟 Cas d'Usage Réels

### Startup avec 5 entités
```
entities/
├── user/
├── company/
├── product/
├── order/
└── invoice/
```

### Entreprise avec 50+ entités
```
entities/
├── customer/
├── order/
├── invoice/
├── payment/
├── shipment/
├── product/
├── inventory/
├── warehouse/
├── supplier/
├── ... (40+ autres)
```

**La structure tient à l'échelle !**

## 🎉 Conclusion

La restructuration par dossiers d'entités apporte :

✅ **Organisation** : Code clair et isolé  
✅ **Scalabilité** : Pattern reproductible  
✅ **Maintenabilité** : Facile à naviguer  
✅ **Collaboration** : Pas de conflits  
✅ **Production** : Architecture professionnelle  

**C'est LA best practice pour les microservices en production !** 🚀🦀✨

---

**Date** : 2025-10-22  
**Impact** : Architecture production-ready  
**Status** : ✅ Complété et testé

