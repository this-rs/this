# Contributing to This-RS 🦀

Merci de votre intérêt pour contribuer à This-RS ! Ce document explique comment participer au projet.

## 🚀 Quick Start

1. **Fork le repository**
2. **Clone votre fork**
   ```bash
   git clone https://github.com/VOTRE_USERNAME/this-rs.git
   cd this-rs
   ```
3. **Créer une branche**
   ```bash
   git checkout -b feature/ma-fonctionnalite
   ```
4. **Faire vos modifications**
5. **Tester**
   ```bash
   cargo test --all-features
   cargo fmt --all -- --check
   cargo clippy --all-targets --all-features -- -D warnings
   ```
6. **Commit et push**
   ```bash
   git add .
   git commit -m "feat: description de ma fonctionnalité"
   git push origin feature/ma-fonctionnalite
   ```
7. **Ouvrir une Pull Request**

## 📋 Types de Contributions

### 🐛 Bug Reports
Ouvrez une issue avec:
- Description claire du bug
- Steps to reproduce
- Version de This-RS
- Version de Rust (`rustc --version`)
- OS et version

### ✨ Feature Requests
Ouvrez une issue avec:
- Description de la fonctionnalité
- Use case concret
- Exemple d'API souhaitée (si applicable)

### 📝 Documentation
- Corriger des typos
- Améliorer les explications
- Ajouter des exemples
- Traduire la documentation

### 💻 Code Contributions
- Nouvelles fonctionnalités
- Corrections de bugs
- Améliorations de performance
- Refactoring

## 🏗️ Architecture du Projet

```
this-rs/
├── src/
│   ├── core/           # Traits et types fondamentaux
│   ├── links/          # Système de gestion des liens
│   ├── config/         # Configuration YAML
│   ├── server/         # ServerBuilder et routing
│   └── entities/       # Macros et helpers
├── examples/           # Exemples d'utilisation
├── docs/              # Documentation
└── tests/             # Tests d'intégration
```

### Principes d'Architecture

1. **Généricité**: Le core ne doit pas référencer de types concrets d'entités
2. **Flexibilité**: Les utilisateurs doivent pouvoir étendre sans modifier le framework
3. **Type Safety**: Utiliser le système de types de Rust au maximum
4. **NoSQL-First**: Design pensé pour DynamoDB/ScyllaDB

## 🧪 Tests

### Exécuter tous les tests
```bash
cargo test --all-features
```

### Tests unitaires
```bash
cargo test --lib
```

### Tests d'intégration
```bash
cargo test --test '*'
```

### Tests des exemples
```bash
cargo test --examples
```

### Doc tests
```bash
cargo test --doc
```

## 🎨 Style de Code

### Formatting
Utilisez `rustfmt`:
```bash
cargo fmt --all
```

### Linting
Utilisez `clippy`:
```bash
cargo clippy --all-targets --all-features -- -D warnings
```

### Conventions de Nommage
- **Types**: `PascalCase` (ex: `EntityDescriptor`)
- **Fonctions/Variables**: `snake_case` (ex: `build_routes`)
- **Constantes**: `SCREAMING_SNAKE_CASE` (ex: `DEFAULT_TIMEOUT`)
- **Traits**: `PascalCase` (ex: `LinkService`)

### Documentation
- Chaque fonction publique doit avoir un doc comment
- Utiliser `///` pour la documentation
- Inclure des exemples dans la doc quand pertinent

Exemple:
```rust
/// Fetches an entity by ID as JSON
///
/// # Arguments
/// * `tenant_id` - The tenant ID for isolation
/// * `entity_id` - The unique ID of the entity
///
/// # Returns
/// The entity serialized as JSON, or an error if not found
///
/// # Example
/// ```ignore
/// let entity = fetcher.fetch_as_json(&tenant_id, &entity_id).await?;
/// ```
async fn fetch_as_json(
    &self,
    tenant_id: &Uuid,
    entity_id: &Uuid,
) -> Result<serde_json::Value>;
```

## 📝 Commit Messages

Suivre la convention [Conventional Commits](https://www.conventionalcommits.org/):

```
<type>(<scope>): <description>

[optional body]

[optional footer]
```

### Types
- `feat`: Nouvelle fonctionnalité
- `fix`: Correction de bug
- `docs`: Documentation uniquement
- `style`: Formatage, point-virgules manquants, etc.
- `refactor`: Refactoring (ni feat ni fix)
- `perf`: Amélioration de performance
- `test`: Ajout ou correction de tests
- `chore`: Maintenance (dependencies, build, etc.)
- `ci`: Modifications CI/CD

### Exemples
```bash
feat(links): add batch fetching for entity enrichment
fix(server): handle missing tenant_id gracefully
docs(readme): update quick start example
refactor(core): simplify EntityRegistry implementation
```

## 🔄 Pull Request Process

1. **Assurez-vous que les tests passent**
   ```bash
   cargo test --all-features
   cargo clippy --all-targets --all-features -- -D warnings
   cargo fmt --all -- --check
   ```

2. **Mettez à jour la documentation** si nécessaire

3. **Ajoutez des tests** pour les nouvelles fonctionnalités

4. **Remplissez le template de PR** avec:
   - Description des changements
   - Issue liée (si applicable)
   - Type de changement (feat/fix/docs/etc.)
   - Checklist des vérifications

5. **Attendez la review**
   - Au moins 1 approbation requise
   - La CI doit passer (tous les jobs verts ✅)

## 🚫 Ce qui N'est PAS Accepté

- ❌ Code non formaté (`cargo fmt`)
- ❌ Warnings clippy non résolus
- ❌ Tests cassés
- ❌ Breaking changes sans discussion préalable
- ❌ Fonctionnalités sans tests
- ❌ Code sans documentation

## 🎯 Roadmap

Consultez les [GitHub Issues](https://github.com/triviere/this-rs/issues) pour voir les tâches planifiées.

### Priorités Actuelles
1. 🚧 Storage abstraction layer
2. 🚧 DynamoDB/ScyllaDB implementation
3. 🚧 Batch operations
4. 📅 Macros procédurales pour entités
5. 📅 Exemples avancés

## 💬 Questions ?

- Ouvrez une [Discussion GitHub](https://github.com/triviere/this-rs/discussions)
- Rejoignez le Discord (lien à venir)
- Contactez les maintainers

## 📜 License

En contribuant à This-RS, vous acceptez que vos contributions soient sous [MIT License](LICENSE-MIT).

---

Merci de contribuer à This-RS ! 🦀✨
