# 🚀 Quick Start Guide - This-RS

## Installation Rapide

```bash
# Cloner le projet
cd this-rs

# Vérifier que tout compile
cargo check

# Lancer les tests
cargo test

# Lancer l'exemple complet
cargo run --example full_api
```

## 📖 Votre Premier Serveur

### 1. Créer votre configuration `links.yaml`

```yaml
entities:
  - singular: user
    plural: users
  
  - singular: car
    plural: cars

links:
  - link_type: owner
    source_type: user
    target_type: car
    forward_route_name: cars-owned
    reverse_route_name: users-owners
    description: "User owns a car"
```

### 2. Définir vos entités

```rust
use this::prelude::*;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct User {
    id: Uuid,
    tenant_id: Uuid,
    name: String,
    email: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Car {
    id: Uuid,
    tenant_id: Uuid,
    brand: String,
    model: String,
}

// La macro génère automatiquement les traits Entity et Data
impl_data_entity!(User, "user", ["name", "email"]);
impl_data_entity!(Car, "car", ["brand", "model"]);
```

### 3. Créer votre serveur Axum

```rust
use this::prelude::*;
use std::sync::Arc;

#[tokio::main]
async fn main() -> Result<()> {
    // Charger la configuration
    let config = Arc::new(LinksConfig::from_yaml_file("links.yaml")?);
    
    // Créer les services
    let link_service: Arc<dyn LinkService> = Arc::new(InMemoryLinkService::new());
    let registry = Arc::new(LinkRouteRegistry::new(config.clone()));
    
    // État de l'application
    let app_state = AppState {
        link_service,
        config,
        registry,
    };
    
    // Construire le routeur
    let app = Router::new()
        // Routes pour lister les liens (forward et reverse)
        .route("/:entity_type/:entity_id/:route_name", 
            get(list_links))
        
        // Routes pour créer et supprimer des liens
        .route("/:source_type/:source_id/:link_type/:target_type/:target_id",
            post(create_link).delete(delete_link))
        
        // Route d'introspection
        .route("/:entity_type/:entity_id/links", 
            get(list_available_links))
        
        .with_state(app_state);
    
    // Lancer le serveur
    let listener = tokio::net::TcpListener::bind("127.0.0.1:3000").await?;
    println!("🚀 Server running on http://127.0.0.1:3000");
    
    axum::serve(listener, app).await?;
    Ok(())
}
```

### 4. Tester votre API

```bash
# Créer un lien
curl -X POST \
  -H "X-Tenant-ID: 550e8400-e29b-41d4-a716-446655440000" \
  -H "Content-Type: application/json" \
  http://localhost:3000/users/USER_ID/owner/cars/CAR_ID

# Lister les voitures possédées par un utilisateur
curl -H "X-Tenant-ID: 550e8400-e29b-41d4-a716-446655440000" \
  http://localhost:3000/users/USER_ID/cars-owned

# Lister les propriétaires d'une voiture
curl -H "X-Tenant-ID: 550e8400-e29b-41d4-a716-446655440000" \
  http://localhost:3000/cars/CAR_ID/users-owners

# Découvrir toutes les routes disponibles
curl -H "X-Tenant-ID: 550e8400-e29b-41d4-a716-446655440000" \
  http://localhost:3000/users/USER_ID/links
```

## 🎯 Cas d'Usage Avancés

### Relations Multiples

Vous pouvez avoir plusieurs types de liens entre les mêmes entités :

```yaml
links:
  # User possède une Car
  - link_type: owner
    source_type: user
    target_type: car
    forward_route_name: cars-owned
    reverse_route_name: users-owners
  
  # User conduit une Car (différent de posséder !)
  - link_type: driver
    source_type: user
    target_type: car
    forward_route_name: cars-driven
    reverse_route_name: users-drivers
```

Cela génère automatiquement :
- `GET /users/{id}/cars-owned` - voitures possédées
- `GET /users/{id}/cars-driven` - voitures conduites
- `GET /cars/{id}/users-owners` - propriétaires
- `GET /cars/{id}/users-drivers` - conducteurs

### Métadonnées sur les Liens

```rust
// Créer un lien avec métadonnées
let metadata = serde_json::json!({
    "role": "Senior Developer",
    "start_date": "2024-01-01",
    "salary": 75000
});

link_service.create(
    &tenant_id,
    "worker",
    EntityReference::new(user_id, "user"),
    EntityReference::new(company_id, "company"),
    Some(metadata),
).await?;
```

### Validation Personnalisée

```yaml
validation_rules:
  owner:
    - source: user
      targets: [car, house, company]
    - source: company
      targets: [car, building]
  
  driver:
    - source: user
      targets: [car, truck]
```

Si vous tentez de créer un lien invalide (ex: `company` driver de `car`), l'API retournera une erreur.

## 📚 Exemples Complets

Le projet contient des exemples fonctionnels :

```bash
# Exemple simple avec données en mémoire
cargo run --example simple_api

# Exemple complet avec serveur Axum
cargo run --example full_api
```

## 🔧 Configuration Avancée

### Pluriels Irréguliers

Le système gère automatiquement les pluriels irréguliers :
- `company` → `companies` ✅
- `address` → `addresses` ✅
- `knife` → `knives` ✅

Mais vous pouvez aussi les spécifier manuellement :

```yaml
entities:
  - singular: person
    plural: people  # Spécifié manuellement
  
  - singular: datum
    plural: data    # Spécifié manuellement
```

### Multi-Tenant Isolation

Toutes les requêtes nécessitent le header `X-Tenant-ID` :

```bash
curl -H "X-Tenant-ID: <votre-tenant-uuid>" \
  http://localhost:3000/users/123/cars-owned
```

Les tenants sont **complètement isolés** :
- Un tenant ne peut jamais voir les données d'un autre
- Impossible d'accéder aux liens d'un autre tenant
- Garantie au niveau du service

## 🎓 Architecture

```
┌─────────────────────────────────────────┐
│         Votre Application               │
│  (User, Car, Company, etc.)             │
└────────────┬────────────────────────────┘
             │
             ▼
┌─────────────────────────────────────────┐
│         This-RS Framework               │
│                                         │
│  ┌──────────┐  ┌──────────────┐       │
│  │  Core    │  │   Links      │       │
│  │ (Generic)│  │  (Agnostic)  │       │
│  └──────────┘  └──────────────┘       │
│                                         │
│  ┌──────────────────────────────┐     │
│  │    HTTP Handlers (Axum)      │     │
│  └──────────────────────────────┘     │
└─────────────────────────────────────────┘
             │
             ▼
┌─────────────────────────────────────────┐
│      Storage (InMemory / PostgreSQL)    │
└─────────────────────────────────────────┘
```

## 📖 Documentation Complète

Pour plus de détails, consultez :
- [README.md](README.md) - Vue d'ensemble complète
- [ARCHITECTURE.md](ARCHITECTURE.md) - Architecture détaillée
- [IMPROVEMENTS.md](IMPROVEMENTS.md) - Dernières améliorations
- [TODO.md](TODO.md) - Roadmap et features à venir

## 💡 Aide et Support

- 📝 Documentation : [docs.rs/this-rs](https://docs.rs/this-rs)
- 🐛 Issues : GitHub Issues
- 💬 Discussions : GitHub Discussions

---

**Prêt à construire votre API ?** 🚀

```bash
cargo run --example full_api
```

