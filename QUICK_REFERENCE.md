# This-RS - Guide de Référence Rapide

## 🚀 Démarrage Rapide

```bash
# Cloner/copier le projet
cd this-rs

# Vérifier que ça compile
make check
# ou: cargo check

# Lancer les tests
make test
# ou: cargo test

# Lancer l'exemple
make run-example
# ou: cargo run --example simple_api

# Voir la documentation
make doc
# ou: cargo doc --open
```

## 📁 Structure du Projet

```
this-rs/
├── 📄 Fichiers de config
│   ├── Cargo.toml          # Dépendances Rust
│   ├── links.yaml          # Configuration des relations
│   └── Makefile            # Commandes utiles
│
├── 📖 Documentation
│   ├── README.md           # Documentation utilisateur
│   ├── GETTING_STARTED.md  # Guide développeur
│   ├── ARCHITECTURE.md     # Architecture détaillée
│   ├── TODO.md             # Roadmap
│   ├── CHECKLIST.md        # Checklist démarrage
│   └── PROJECT_SUMMARY.md  # Résumé du projet
│
├── 🔧 Source
│   ├── src/lib.rs          # Point d'entrée
│   ├── src/core/           # Framework générique (6 fichiers)
│   ├── src/links/          # Gestion relations (3 fichiers)
│   ├── src/entities/       # Macros entités (2 fichiers)
│   └── src/config/         # Config YAML (1 fichier)
│
└── 📚 Exemples
    └── examples/simple_api.rs
```

## 🔧 Commandes Make

```bash
make help           # Afficher l'aide
make check          # Vérifier compilation
make test           # Lancer tests
make test-verbose   # Tests avec output
make build          # Compiler (debug)
make build-release  # Compiler (optimisé)
make run-example    # Lancer exemple
make doc            # Générer docs
make fmt            # Formater code
make clippy         # Linter
make clean          # Nettoyer
make coverage       # Rapport couverture
make all            # fmt + check + clippy + test
make watch          # Mode auto-reload
```

## 📦 Commandes Cargo Importantes

```bash
# Compilation
cargo check                    # Vérifier sans compiler
cargo build                    # Compiler (debug)
cargo build --release          # Compiler (optimisé)
cargo clean                    # Nettoyer

# Tests
cargo test                     # Tous les tests
cargo test test_name           # Test spécifique
cargo test -- --nocapture      # Avec output
cargo test --lib               # Tests lib seulement
cargo test --test test_file    # Tests d'intégration

# Documentation
cargo doc                      # Générer docs
cargo doc --open               # Générer et ouvrir
cargo doc --no-deps            # Sans dépendances

# Qualité du code
cargo fmt                      # Formater
cargo fmt --check              # Vérifier format
cargo clippy                   # Linter
cargo clippy -- -D warnings    # Linter strict

# Exemples
cargo run --example simple_api # Lancer exemple
cargo build --example simple_api # Compiler exemple

# Dépendances
cargo update                   # Mettre à jour
cargo tree                     # Arbre dépendances
cargo audit                    # Audit sécurité (install first)

# Benchmarks (quand impl)
cargo bench                    # Lancer benchmarks

# Publication
cargo publish --dry-run        # Test publication
cargo publish                  # Publier sur crates.io
```

## 🛠️ Outils Utiles à Installer

```bash
# Coverage
cargo install cargo-tarpaulin

# Auto-reload
cargo install cargo-watch

# Expansion de macros
cargo install cargo-expand

# Audit de sécurité
cargo install cargo-audit

# Benchmarking
cargo install cargo-criterion
```

## 📝 Créer une Nouvelle Entité

### 1. Définir la Struct

```rust
// Dans un nouveau fichier ou dans main.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
struct MyEntity {
    id: Uuid,
    tenant_id: Uuid,
    field1: String,
    field2: i32,
}
```

### 2. Implémenter les Traits

```rust
// Utiliser la macro
impl_data_entity!(MyEntity, "my_entity", ["field1", "field2"]);
```

### 3. Configurer dans YAML

```yaml
# Dans links.yaml
entities:
  - singular: my_entity
    plural: my_entities

links:
  - link_type: related
    source_type: user
    target_type: my_entity
    forward_route_name: my_entities-related
    reverse_route_name: users-related
```

### 4. Utiliser

```rust
let service = InMemoryLinkService::new();

service.create(
    &tenant_id,
    "related",
    EntityReference::new(user_id, "user"),
    EntityReference::new(entity_id, "my_entity"),
    None,
).await?;
```

## 🔗 Types de Relations

### Relation Simple

```yaml
links:
  - link_type: owner
    source_type: user
    target_type: car
    forward_route_name: cars-owned
    reverse_route_name: users-owners
```

**Routes générées:**
- `GET /users/{id}/cars-owned`
- `GET /cars/{id}/users-owners`

### Relations Multiples (Mêmes Entités)

```yaml
links:
  # Relation 1
  - link_type: owner
    source_type: user
    target_type: car
    forward_route_name: cars-owned
    reverse_route_name: users-owners
  
  # Relation 2 (différente!)
  - link_type: driver
    source_type: user
    target_type: car
    forward_route_name: cars-driven
    reverse_route_name: users-drivers
```

### Relation avec Métadonnées

```yaml
links:
  - link_type: worker
    source_type: user
    target_type: company
    forward_route_name: companies-work
    reverse_route_name: users-workers
    required_fields:
      - role
      - start_date
```

```rust
// Usage avec metadata
service.create(
    &tenant_id,
    "worker",
    EntityReference::new(user_id, "user"),
    EntityReference::new(company_id, "company"),
    Some(serde_json::json!({
        "role": "Developer",
        "start_date": "2024-01-01"
    })),
).await?;
```

## 🔍 Requêtes Courantes

### Trouver tous les liens d'une source

```rust
let links = link_service.find_by_source(
    &tenant_id,
    &user_id,
    "user",
    None,              // Tous les link_types
    None,              // Tous les target_types
).await?;
```

### Trouver liens spécifiques

```rust
// Toutes les voitures possédées par l'utilisateur
let owned_cars = link_service.find_by_source(
    &tenant_id,
    &user_id,
    "user",
    Some("owner"),     // Uniquement type "owner"
    Some("car"),       // Uniquement vers "car"
).await?;
```

### Trouver depuis la target (reverse)

```rust
// Tous les propriétaires d'une voiture
let owners = link_service.find_by_target(
    &tenant_id,
    &car_id,
    "car",
    Some("owner"),
    Some("user"),
).await?;
```

## 🧪 Tests

### Test Unitaire

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_my_function() {
        let result = my_function();
        assert_eq!(result, expected);
    }
}
```

### Test Async

```rust
#[tokio::test]
async fn test_async_function() {
    let result = async_function().await.unwrap();
    assert_eq!(result, expected);
}
```

### Lancer un test spécifique

```bash
cargo test test_name
cargo test test_name -- --nocapture
```

## 🐛 Debugging

### Afficher les valeurs

```rust
println!("{:?}", value);          // Debug
println!("{:#?}", value);         // Pretty Debug
dbg!(value);                      // Debug macro
```

### Logs avec tracing

```rust
use tracing::{info, warn, error};

info!("Info message");
warn!("Warning: {}", message);
error!("Error occurred: {:?}", err);
```

### Expansion de macros

```bash
cargo expand module::path
```

## 📊 Validation de Code

### Format

```bash
# Formater automatiquement
cargo fmt

# Vérifier le format (CI)
cargo fmt --check
```

### Linter

```bash
# Suggestions d'amélioration
cargo clippy

# Mode strict (pour CI)
cargo clippy -- -D warnings
```

### Coverage

```bash
# Installer (une fois)
cargo install cargo-tarpaulin

# Générer rapport
cargo tarpaulin --out Html

# Ouvrir le rapport
open target/tarpaulin/index.html
```

## 🔐 Multi-Tenant

Toutes les opérations nécessitent un `tenant_id`:

```rust
// Création
service.create(&tenant_id, ...);

// Lecture
service.get(&tenant_id, &id);
service.list(&tenant_id);

// Recherche
service.find_by_source(&tenant_id, ...);

// Suppression
service.delete(&tenant_id, &id);
```

**Garantie:** Un tenant ne peut jamais accéder aux données d'un autre.

## 🚨 Erreurs Courantes

### "Cannot find macro impl_data_entity"

**Solution:** Vérifier les imports
```rust
use this::prelude::*;
```

### "Trait bounds not satisfied"

**Solution:** Vérifier que le type implémente les bons traits
```rust
// Doit avoir
#[derive(Debug, Clone, Serialize, Deserialize)]
```

### "Method leak not found"

**Solution:** Problème dans la macro, voir CHECKLIST.md

## 📚 Ressources

### Documentation Locale
```bash
make doc
# ou
cargo doc --open
```

### Documentation En Ligne
- README.md - Vue d'ensemble
- GETTING_STARTED.md - Guide développeur
- ARCHITECTURE.md - Architecture détaillée
- TODO.md - Roadmap et tâches
- CHECKLIST.md - Checklist démarrage

### Communauté Rust
- [The Rust Book](https://doc.rust-lang.org/book/)
- [Rust by Example](https://doc.rust-lang.org/rust-by-example/)
- [Axum Guide](https://docs.rs/axum/latest/axum/)
- [SQLx Guide](https://docs.rs/sqlx/latest/sqlx/)

## 🎯 Raccourcis IDE

### VS Code

```json
{
    "rust-analyzer.checkOnSave.command": "clippy",
    "rust-analyzer.cargo.features": "all"
}
```

### Shortcuts
- `F12` - Go to definition
- `Shift+F12` - Find references
- `Ctrl+Space` - Auto-complete
- `F2` - Rename symbol

## 💡 Tips & Tricks

### Watch mode (auto-reload)

```bash
cargo install cargo-watch
cargo watch -x check -x test
```

### Tests rapides

```bash
# Seulement tests qui ont changé
cargo test --lib

# Parallélisme
cargo test -- --test-threads=4
```

### Documentation privée

```rust
/// Documentation publique
pub fn function() {}

// Documentation privée
fn internal_function() {}
```

### Macros debugging

```bash
cargo expand src/module.rs
```

---

**Version:** 0.1.0  
**Dernière mise à jour:** 2025-10-22
