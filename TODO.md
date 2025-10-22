# This-RS - TODO & Roadmap

## 🔥 Phase 1: Faire Compiler (Critique)

### Bugs à Corriger
- [ ] Fixer la macro `impl_data_entity!` 
  - Problème: `.leak()` sur String dynamique
  - Solution: Utiliser `Box::leak` ou approche différente
- [ ] Vérifier tous les imports manquants
- [ ] Tester que tous les tests passent
- [ ] S'assurer que l'exemple compile

### Tests de Base
- [ ] Vérifier `cargo check` passe
- [ ] Vérifier `cargo test` passe
- [ ] Vérifier `cargo run --example simple_api` fonctionne
- [ ] Tester avec `cargo clippy`
- [ ] Formater avec `cargo fmt`

## ⚡ Phase 2: Core Features (Haute Priorité)

### Extracteurs HTTP (core/extractors.rs)
- [ ] Implémenter `DataExtractor<T>` pour Axum
  - [ ] Extraction du body JSON
  - [ ] Validation automatique
  - [ ] Extraction du tenant_id depuis headers
- [ ] Implémenter `LinkExtractor`
- [ ] Ajouter tests pour les extracteurs

### Handlers HTTP (links/handlers.rs)
- [ ] Handler: Créer un lien
  ```
  POST /users/{id}/{link_type}/cars/{target_id}
  ```
- [ ] Handler: Lister liens forward
  ```
  GET /users/{id}/cars-owned
  ```
- [ ] Handler: Lister liens reverse
  ```
  GET /cars/{id}/users-owners
  ```
- [ ] Handler: Supprimer un lien
  ```
  DELETE /links/{link_id}
  ```
- [ ] Handler: Introspection
  ```
  GET /users/{id}/links
  ```
- [ ] Tests d'intégration HTTP

### Macro CRUD (entities/macros.rs)
- [ ] Améliorer `impl_data_entity!`
- [ ] Créer `impl_crud_handlers!` (macro procédurale?)
  - [ ] Générer handler `list`
  - [ ] Générer handler `get`
  - [ ] Générer handler `create`
  - [ ] Générer handler `update`
  - [ ] Générer handler `delete`
- [ ] Documentation des macros avec exemples

## 🗄️ Phase 3: Database Support (Priorité Moyenne)

### PostgreSQL LinkService
- [ ] Créer `links/postgres_service.rs`
- [ ] Implémenter `PostgresLinkService`
- [ ] Créer schéma SQL
  ```sql
  CREATE TABLE links (...)
  CREATE INDEX idx_source ...
  CREATE INDEX idx_target ...
  ```
- [ ] Migrations SQLx
- [ ] Tests d'intégration avec PostgreSQL

### Support pour DataService
- [ ] Template pour PostgreSQL DataService
- [ ] Exemple d'implémentation complète
- [ ] Tests d'intégration

## 📚 Phase 4: Exemples & Documentation (Priorité Moyenne)

### Exemples Supplémentaires
- [ ] `examples/full_rest_api.rs`
  - Serveur Axum complet
  - Routes CRUD pour User/Car
  - Routes pour liens
  - Middleware tenant_id
  - Gestion erreurs
- [ ] `examples/with_postgres.rs`
  - Setup base de données
  - Migrations
  - CRUD avec PostgreSQL
- [ ] `examples/multi_tenant.rs`
  - Démonstration isolation tenants
  - Multiples tenants en parallèle

### Documentation
- [ ] Documenter tous les types publics
- [ ] Ajouter plus d'exemples dans doc comments
- [ ] Créer guide "How to create your first entity"
- [ ] Créer guide "How to define relationships"
- [ ] FAQ avec cas d'usage courants

## 🔧 Phase 5: Developer Tools (Priorité Basse)

### CLI Tool
- [ ] Créer `this-cli` crate
- [ ] Command: `this new my-project`
  - Génère structure projet
  - Crée links.yaml
  - Setup Cargo.toml
- [ ] Command: `this entity User name:String email:String`
  - Génère struct + impl
  - Ajoute à links.yaml
- [ ] Command: `this link User Car owner`
  - Ajoute relation dans links.yaml

### Code Generation
- [ ] Générateur de clients TypeScript
- [ ] Générateur de clients Python
- [ ] Générateur documentation OpenAPI/Swagger

## 🚀 Phase 6: Advanced Features (Long Terme)

### Validation & Rules
- [ ] Validateurs de règles métier via YAML
  ```yaml
  links:
    - link_type: owner
      rules:
        - max_targets: 5  # Un user peut posséder max 5 cars
        - unique: true    # Un lien unique par paire
  ```
- [ ] Hook système (before_create, after_create, etc.)
- [ ] Validateurs custom programmables

### GraphQL Support
- [ ] Intégration avec async-graphql
- [ ] Générateur de schéma GraphQL
- [ ] Queries et mutations automatiques
- [ ] Subscriptions pour changements en temps réel

### Performance
- [ ] Système de cache (Redis?)
- [ ] Batch operations
- [ ] Query optimization
- [ ] Benchmarks
  ```
  cargo bench
  ```

### Admin UI
- [ ] Interface web générique
- [ ] Visualisation des entités
- [ ] Visualisation graphe des relations
- [ ] Éditeur YAML avec auto-complétion

## 🔍 Phase 7: Quality & Production (Continu)

### Tests
- [ ] Coverage > 80%
  ```
  cargo tarpaulin
  ```
- [ ] Tests de charge (criterion)
- [ ] Tests end-to-end
- [ ] Tests de régression

### CI/CD
- [ ] GitHub Actions
  - Build sur Linux/Mac/Windows
  - Tests automatiques
  - Clippy & format check
  - Coverage report
- [ ] Semantic versioning
- [ ] Automated releases

### Security
- [ ] Audit de sécurité
  ```
  cargo audit
  ```
- [ ] Input sanitization
- [ ] SQL injection prevention
- [ ] Rate limiting

### Documentation
- [ ] Guide de migration de versions
- [ ] Changelog détaillé
- [ ] Best practices guide
- [ ] Architecture decision records (ADR)

## 📦 Phase 8: Publication (Quand prêt)

### Préparation
- [ ] README.md finalisé
- [ ] LICENSE vérifiée
- [ ] CHANGELOG.md créé
- [ ] Version 0.1.0 stable
- [ ] Documentation complète

### Publication
- [ ] Publier sur crates.io
  ```
  cargo publish
  ```
- [ ] Annoncer sur:
  - [ ] Reddit (r/rust)
  - [ ] This Week in Rust
  - [ ] Blog post
- [ ] Créer GitHub repository
- [ ] Setup discussions/issues

## 🎯 Objectifs de Qualité

### Code Quality
- Coverage: > 80%
- Clippy: 0 warnings
- Docs: 100% des APIs publiques
- Examples: Au moins 3 exemples fonctionnels

### Performance Targets
- Link creation: < 1ms (in-memory)
- Link query: < 5ms (in-memory)
- API response: < 50ms (p99)

### Developer Experience
- Setup nouveau projet: < 5 minutes
- Ajouter nouvelle entité: < 5 minutes
- Ajouter nouvelle relation: < 2 minutes (juste YAML)

---

## 📝 Notes

### Décisions Techniques
- String-based types > Enums (extensibilité)
- Configuration YAML > Code (flexibilité)
- Macros pour boilerplate (DRY)
- Async/await partout (performance)

### Non-Goals (Ce qu'on ne fait PAS)
- ❌ ORM complet (trop complexe)
- ❌ Frontend framework (hors scope)
- ❌ Support NoSQL (pour l'instant)
- ❌ Migrations automatiques (trop magique)

---

**Dernière mise à jour:** 2025-10-22
