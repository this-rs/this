# Architecture This-RS Framework

## 📐 Vue d'Ensemble

```
┌─────────────────────────────────────────────────────────────┐
│                     USER APPLICATION                        │
│                                                             │
│  ┌──────────┐  ┌──────────┐  ┌──────────┐                │
│  │   User   │  │ Company  │  │   Car    │  ... Entities  │
│  └────┬─────┘  └────┬─────┘  └────┬─────┘                │
│       │             │              │                        │
│       └─────────────┴──────────────┘                       │
│                     │                                       │
└─────────────────────┼───────────────────────────────────────┘
                      │
                      ▼
┌─────────────────────────────────────────────────────────────┐
│                   THIS-RS FRAMEWORK                         │
│                                                             │
│  ┌───────────────────────────────────────────────────┐    │
│  │              CORE MODULE (Generic)                 │    │
│  │                                                    │    │
│  │  ┌──────────┐  ┌──────────┐  ┌──────────┐      │    │
│  │  │  Entity  │  │  Link    │  │  Field   │      │    │
│  │  │  Traits  │  │  System  │  │  System  │      │    │
│  │  └──────────┘  └──────────┘  └──────────┘      │    │
│  │                                                    │    │
│  │  ┌──────────┐  ┌──────────┐                      │    │
│  │  │ Service  │  │Pluralize │                      │    │
│  │  │  Traits  │  │  System  │                      │    │
│  │  └──────────┘  └──────────┘                      │    │
│  └───────────────────────────────────────────────────┘    │
│                           │                                 │
│       ┌───────────────────┼───────────────────┐           │
│       ▼                   ▼                   ▼           │
│  ┌─────────┐      ┌─────────────┐      ┌─────────┐      │
│  │ LINKS   │      │   CONFIG    │      │ENTITIES │      │
│  │ MODULE  │◄─────┤   MODULE    │─────►│ MODULE  │      │
│  │         │      │             │      │         │      │
│  │ Service │      │ YAML Loader │      │ Macros  │      │
│  │Registry │      │   Parser    │      │         │      │
│  └─────────┘      └─────────────┘      └─────────┘      │
│                                                             │
└─────────────────────────────────────────────────────────────┘
                      │
                      ▼
            ┌──────────────────┐
            │   STORAGE LAYER  │
            │                  │
            │  ┌────────────┐  │
            │  │  In-Memory │  │
            │  └────────────┘  │
            │  ┌────────────┐  │
            │  │ PostgreSQL │  │ (à venir)
            │  └────────────┘  │
            └──────────────────┘
```

## 🏗️ Modules Détaillés

### 1. Core Module (Générique)

Le cœur du framework, totalement agnostique des types d'entités.

```
src/core/
├── entity.rs       ← Traits fondamentaux
│   ├── Entity      : Métadonnées (nom, service)
│   └── Data        : Entités concrètes (id, tenant, fields)
│
├── link.rs         ← Système de liens polymorphes
│   ├── Link           : Relation entre 2 entités
│   ├── EntityReference: Référence dynamique (id + type)
│   └── LinkDefinition : Configuration d'un type de lien
│
├── field.rs        ← Types et validation
│   ├── FieldValue     : Valeur polymorphe (String, Int, UUID, etc.)
│   └── FieldFormat    : Validateurs (Email, URL, Phone, Custom)
│
├── service.rs      ← Traits de service
│   ├── DataService<T> : CRUD pour entités
│   └── LinkService    : CRUD pour liens
│
├── pluralize.rs    ← Gestion pluriels
│   └── Pluralizer     : company → companies
│
└── extractors.rs   ← Extracteurs HTTP (Axum)
    ├── DataExtractor<T>
    └── LinkExtractor
```

**Principe clé:** Aucune référence aux types concrets (User, Car, etc.)

### 2. Links Module (Agnostique)

Gestion des relations entre entités, sans connaître les types.

```
src/links/
├── service.rs      ← Implémentations LinkService
│   └── InMemoryLinkService : Stockage en mémoire
│
└── registry.rs     ← Résolution de routes
    └── LinkRouteRegistry : URL → LinkDefinition
```

**Routes générées:**
```
/users/{id}/cars-owned      → Forward (user → car, owner)
/cars/{id}/users-owners     → Reverse (car → user, owner)
```

### 3. Config Module

Chargement et parsing de la configuration YAML.

```
src/config/
└── mod.rs
    ├── LinksConfig    : Configuration complète
    └── EntityConfig   : Config d'une entité
```

**Fichier YAML:**
```yaml
entities:
  - singular: user
    plural: users

links:
  - link_type: owner
    source_type: user
    target_type: car
    forward_route_name: cars-owned
    reverse_route_name: users-owners
```

### 4. Entities Module

Macros et helpers pour créer des entités facilement.

```
src/entities/
└── macros.rs
    └── impl_data_entity! : Génère trait implementations
```

**Usage:**
```rust
impl_data_entity!(User, "user", ["name", "email"]);
```

**Génère:**
```rust
impl Entity for User { ... }
impl Data for User { ... }
```

## 🔄 Flux de Données

### Création d'un Lien

```
1. HTTP Request
   POST /users/123/owner/cars/456

2. Axum Handler
   ↓ Parse URL params

3. LinkService.create()
   ↓ tenant_id: UUID
   ↓ link_type: "owner"
   ↓ source: EntityReference(123, "user")
   ↓ target: EntityReference(456, "car")

4. Storage Layer
   ↓ Insert Link

5. Response
   ← Link { id, tenant_id, link_type, ... }
```

### Requête de Liens

```
1. HTTP Request
   GET /users/123/cars-owned

2. LinkRouteRegistry.resolve_route()
   ↓ "user" + "cars-owned"
   ↓ → LinkDefinition { link_type: "owner", ... }
   ↓ → Direction: Forward

3. LinkService.find_by_source()
   ↓ tenant_id, source_id: 123
   ↓ source_type: "user"
   ↓ link_type: Some("owner")
   ↓ target_type: Some("car")

4. Storage Layer
   ↓ Query links matching criteria

5. Response
   ← Vec<Link>
```

## 🎯 Principes d'Architecture

### 1. Séparation des Responsabilités

```
Core      : Abstractions génériques
Links     : Gestion des relations (agnostique)
Entities  : Code spécifique aux entités
Config    : Configuration et métadonnées
```

### 2. Polymorphisme par String

```rust
// ❌ Pas ça (couplé)
enum EntityType { User, Car, Company }

// ✅ Ça (découplé)
struct EntityReference {
    id: Uuid,
    entity_type: String,  // "user", "car", "company", ...
}
```

**Avantage:** Ajouter une entité ne modifie pas le framework.

### 3. Configuration > Code

```yaml
# Ajouter une relation = éditer YAML
links:
  - link_type: driver
    source_type: user
    target_type: car
    # ...
```

**Pas besoin de toucher au code Rust !**

### 4. Traits Génériques

```rust
// Service fonctionne pour TOUT type T: Data
trait DataService<T: Data> {
    async fn create(&self, tenant_id: &Uuid, entity: T) -> Result<T>;
    // ...
}

// Implémente une seule fois, marche pour tous les types
impl<T: Data> DataService<T> for MyService { ... }
```

## 🔐 Sécurité Multi-Tenant

```
┌──────────────────┐
│   Request        │
│   Headers:       │
│   X-Tenant-ID    │
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│   Middleware     │
│   Extract        │
│   tenant_id      │
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│   All queries    │
│   filtered by    │
│   tenant_id      │
└──────────────────┘
```

**Garanties:**
- Isolation complète entre tenants
- Impossible d'accéder aux données d'un autre tenant
- Pas de requêtes SQL sans filtre tenant_id

## 📊 Modèle de Données

### Entité (User, Car, Company)

```
┌─────────────────────┐
│ Data Entity         │
├─────────────────────┤
│ id: UUID            │
│ tenant_id: UUID     │
│ ... fields ...      │
└─────────────────────┘
```

### Lien (Owner, Driver, Worker)

```
┌─────────────────────────────┐
│ Link                        │
├─────────────────────────────┤
│ id: UUID                    │
│ tenant_id: UUID             │
│ link_type: String           │
│ source: EntityReference     │
│   ├─ id: UUID               │
│   └─ entity_type: String    │
│ target: EntityReference     │
│   ├─ id: UUID               │
│   └─ entity_type: String    │
│ metadata: JSON (optional)   │
│ created_at: DateTime        │
│ updated_at: DateTime        │
└─────────────────────────────┘
```

### Exemple Concret

```
User (id: 123)
  ├─ owner ──→ Car (id: 456)
  ├─ driver ──→ Car (id: 456)  ← Même voiture, relation différente!
  └─ worker ──→ Company (id: 789)

Car (id: 456)
  ├─ owner ←── User (id: 123)
  └─ driver ←── User (id: 123)
```

## 🚀 Extensibilité

### Ajouter une Nouvelle Entité

**1. Définir la struct (5 lignes)**
```rust
#[derive(Serialize, Deserialize)]
struct Dragon {
    id: Uuid,
    tenant_id: Uuid,
    name: String,
}
```

**2. Implémenter les traits (1 ligne avec macro)**
```rust
impl_data_entity!(Dragon, "dragon", ["name"]);
```

**3. Configurer les relations (YAML)**
```yaml
entities:
  - singular: dragon
    plural: dragons

links:
  - link_type: rider
    source_type: user
    target_type: dragon
    # ...
```

**C'est tout !** Le framework gère automatiquement:
- Routes HTTP
- CRUD operations
- Relations bidirectionnelles
- Validation
- Multi-tenant

## 🎨 Design Patterns Utilisés

1. **Repository Pattern** : DataService, LinkService
2. **Strategy Pattern** : Multiple implémentations de LinkService
3. **Builder Pattern** : Link::new()
4. **Registry Pattern** : LinkRouteRegistry
5. **Trait Objects** : Polymorphisme via traits
6. **Type State** : Compile-time guarantees via traits

## 📈 Scalabilité

```
Application
    ↓
This-RS Framework (logic métier)
    ↓
Multiple Storage Backends
    ├─ In-Memory (dev/test)
    ├─ PostgreSQL (production)
    ├─ MySQL (si besoin)
    └─ MongoDB (NoSQL, futur)
```

**Principe:** Le framework ne dépend pas du storage.

---

**Version:** 0.1.0  
**Dernière mise à jour:** 2025-10-22
