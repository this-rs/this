# Contributing to this-rs

Thank you for your interest in contributing to this-rs! This document provides guidelines and instructions for contributing.

## 🎯 Code of Conduct

Be respectful, inclusive, and constructive. We're building something great together!

## 🚀 Getting Started

### Prerequisites

- Rust 1.70 or higher
- Git
- Basic knowledge of Rust and async programming

### Setup

```bash
# Clone the repository
git clone https://github.com/your-org/this-rs.git
cd this-rs

# Check that everything compiles
cargo check

# Run tests
cargo test

# Run examples
cargo run --example microservice
```

## 📝 How to Contribute

### 1. Reporting Bugs

**Before submitting:**
- Check if the bug has already been reported in [Issues](https://github.com/your-org/this-rs/issues)
- Make sure you're using the latest version

**Bug Report Template:**
```markdown
**Describe the bug**
A clear and concise description of what the bug is.

**To Reproduce**
Steps to reproduce the behavior:
1. Create entity '...'
2. Create link '...'
3. Query '...'
4. See error

**Expected behavior**
What you expected to happen.

**Actual behavior**
What actually happened.

**Environment:**
- OS: [e.g., macOS 14.0]
- Rust version: [e.g., 1.75.0]
- this-rs version: [e.g., 0.0.2]

**Additional context**
Add any other context about the problem here.
```

### 2. Suggesting Features

**Feature Request Template:**
```markdown
**Is your feature request related to a problem?**
A clear description of the problem. Ex. I'm always frustrated when [...]

**Describe the solution you'd like**
A clear description of what you want to happen.

**Describe alternatives you've considered**
Other solutions or features you've considered.

**Additional context**
Any other context or screenshots about the feature request.
```

### 3. Submitting Code

#### Fork and Clone

```bash
# Fork the repository on GitHub, then clone your fork
git clone https://github.com/YOUR_USERNAME/this-rs.git
cd this-rs

# Add upstream remote
git remote add upstream https://github.com/your-org/this-rs.git
```

#### Create a Branch

```bash
# Create a feature branch
git checkout -b feature/your-feature-name

# Or a bugfix branch
git checkout -b fix/bug-description
```

#### Make Your Changes

1. **Write Code**
   - Follow Rust conventions
   - Add tests for new functionality
   - Update documentation

2. **Run Tests**
   ```bash
   cargo test
   cargo clippy -- -D warnings
   cargo fmt --check
   ```

3. **Commit Changes**
   ```bash
   git add .
   git commit -m "feat: add new feature X"
   ```

   **Commit Message Format:**
   ```
   <type>: <description>

   [optional body]

   [optional footer]
   ```

   **Types:**
   - `feat`: New feature
   - `fix`: Bug fix
   - `docs`: Documentation only
   - `style`: Code style changes (formatting, etc.)
   - `refactor`: Code refactoring
   - `test`: Adding or updating tests
   - `chore`: Maintenance tasks

#### Submit a Pull Request

```bash
# Push to your fork
git push origin feature/your-feature-name
```

Then create a Pull Request on GitHub with:
- Clear title and description
- Reference to related issues (if any)
- Screenshots/examples (if applicable)

## 🏗️ Architecture Guidelines

### Adding a New Entity

To add a new entity to the examples:

1. **Create Entity Folder**
   ```
   examples/microservice/entities/new_entity/
   ├── mod.rs
   ├── model.rs       # Use impl_data_entity! macro
   ├── store.rs       # Implement EntityFetcher + EntityCreator
   ├── handlers.rs    # HTTP handlers
   └── descriptor.rs  # EntityDescriptor implementation
   ```

2. **Use Macro for Entity Definition**
   ```rust
   // model.rs
   use this::prelude::*;

   impl_data_entity!(NewEntity, "new_entity", ["name", "field"], {
       field: String,
       another_field: i32,
   });
   ```

3. **Implement Fetcher and Creator**
   ```rust
   // store.rs
   #[async_trait]
   impl EntityFetcher for NewEntityStore {
       async fn fetch_as_json(&self, entity_id: &Uuid) -> Result<serde_json::Value> {
           // Implementation
       }
   }

   #[async_trait]
   impl EntityCreator for NewEntityStore {
       async fn create_from_json(&self, entity_data: serde_json::Value) -> Result<serde_json::Value> {
           // Implementation
       }
   }
   ```

4. **Register in Module**
   ```rust
   impl Module for YourModule {
       fn register_entities(&self, registry: &mut EntityRegistry) {
           registry.register(Box::new(NewEntityDescriptor::new(...)));
       }

       fn get_entity_fetcher(&self, entity_type: &str) -> Option<Arc<dyn EntityFetcher>> {
           match entity_type {
               "new_entity" => Some(Arc::new(self.store.new_entities.clone())),
               _ => None,
           }
       }

       fn get_entity_creator(&self, entity_type: &str) -> Option<Arc<dyn EntityCreator>> {
           match entity_type {
               "new_entity" => Some(Arc::new(self.store.new_entities.clone())),
               _ => None,
           }
       }
   }
   ```

### Adding a New Storage Backend

To add a new storage backend (e.g., PostgreSQL, MongoDB):

1. **Create Storage Module**
   ```
   src/storage/
   └── postgresql.rs  # or mongodb.rs
   ```

2. **Implement Traits**
   ```rust
   pub struct PostgresDataService<T: Data> {
       pool: Pool<Postgres>,
       _phantom: PhantomData<T>,
   }

   #[async_trait]
   impl<T: Data> DataService<T> for PostgresDataService<T> {
       async fn create(&self, entity: T) -> Result<T> {
           // Implementation
       }
       // ... other methods
   }

   pub struct PostgresLinkService {
       pool: Pool<Postgres>,
   }

   #[async_trait]
   impl LinkService for PostgresLinkService {
       async fn create(&self, link: LinkEntity) -> Result<LinkEntity> {
           // Implementation
       }
       // ... other methods
   }
   ```

3. **Add Tests**
   ```rust
   #[cfg(test)]
   mod tests {
       use super::*;

       #[tokio::test]
       async fn test_create_entity() {
           // Test implementation
       }
   }
   ```

## ✅ Code Quality Standards

### Le hook de pre-push (recommandé)

Le dépôt en fournit un, versionné, qui exécute exactement ce que la CI exécute :

```bash
git config core.hooksPath scripts/hooks
```

**Pourquoi pas `cargo fmt --check` tout seul ?** Il ne voit que les membres du workspace.
Le job Rustfmt formate **tous les fichiers suivis** (`git ls-files '*.rs'`), et `this-ui`,
`this-client-web`, `this-device-tauri` et `this-shell-tauri` sont volontairement hors du
workspace (cibles wasm et Tauri). Une PR est déjà partie rouge avec un `cargo fmt --check`
local vert pour cette seule raison.

**Pourquoi la version de rustfmt compte ?** Aucune toolchain n'est épinglée : la CI suit le
stable du jour, et deux rustfmt séparés de quelques mois ne rangent pas les `use` de la même
façon. Si le hook réclame un fichier que tu viens de formater, `rustup update stable`.

**Et la version de clippy, autant.** Une nouvelle règle arrive à chaque stable, et elle peut
rougir `main` sans qu'aucun contrôle local ne le dise : mesuré le 13/11/2026, `needless_bool`
apparu en 1.100 alors que la machine était en 1.97.1 — la CI était rouge, le hook vert. Le
remède est le même (`rustup update stable`), ou, pour ne pas déplacer la toolchain des autres
sessions, une installation à côté : `rustup toolchain install 1.100.0 --profile minimal
--component clippy` puis `cargo +1.100.0 clippy …`.

### Plusieurs sessions, une seule copie de travail

Ce dépôt se construit souvent avec deux agents (ou plus) sur le **même** répertoire et la même
branche. Ce n'est pas un problème tant que trois règles sont tenues — chacune vient d'un incident
mesuré le 13/11/2026 :

1. **`git add -A` est interdit.** Une session a committé l'avancement non commité d'une autre :
   un `mod linux;` sans son répertoire, et trente secondes plus tard un test rouge sur un commit
   qui avait l'air propre. On stage **par nom de fichier**, et pour un fichier partagé on ne
   stage que ses propres hunks (`git show HEAD:… > /tmp/f`, on édite, `git hash-object -w`,
   `git update-index --cacheinfo 100644,<sha>,<chemin>` — la copie de travail ne bouge pas).
2. **Compiler le commit, pas la copie de travail.** Après un commit partiel, la copie contient
   des choses qui ne sont pas dans le commit. `git worktree add ../_wt-verif <sha>` puis
   `cargo test` dans ce worktree ; c'est le seul contrôle qui dit si la branche tient.
3. **`--force-with-lease`, jamais `--force`, et seulement sur son propre sommet.** Réécrire un
   commit déjà poussé que l'autre session a repris casse son travail sans qu'elle le voie.

Deux corollaires : le hook de pre-push est le garde-fou **commun** (il ne vérifie que ce que la
branche touche), et un worktree par session reste plus sûr qu'un `git checkout` partagé.

### Testing

- Write unit tests for all new functionality
- Aim for at least 80% code coverage
- Include integration tests for examples

```bash
# Run all tests
cargo test

# Run specific test
cargo test test_name

# Run with output
cargo test -- --nocapture
```

### Linting

```bash
# Run clippy
cargo clippy -- -D warnings

# Auto-fix what can be fixed
cargo clippy --fix
```

### Formatting

```bash
# Check formatting
cargo fmt --check

# Format code
cargo fmt
```

### Documentation

- Add doc comments to public APIs
- Update README.md if adding features
- Add examples for new functionality

```rust
/// Creates a new entity of type T.
///
/// # Arguments
///
/// * `entity` - The entity to create
///
/// # Returns
///
/// The created entity with generated ID and timestamps
///
/// # Examples
///
/// ```
/// let order = Order::new("ORD-123", "active", 1500.00);
/// let created = service.create(order).await?;
/// ```
pub async fn create(&self, entity: T) -> Result<T> {
    // Implementation
}
```

## 🔍 Review Process

1. **Automated Checks**: CI runs tests, clippy, and formatting checks
2. **Code Review**: A maintainer reviews your code
3. **Discussion**: Address any feedback or questions
4. **Approval**: Once approved, your PR will be merged

## 📚 Resources

- [Rust Book](https://doc.rust-lang.org/book/)
- [Async Book](https://rust-lang.github.io/async-book/)
- [Axum Documentation](https://docs.rs/axum/)
- [Project Documentation](docs/)

## 🎁 Recognition

Contributors will be:
- Listed in CONTRIBUTORS.md
- Mentioned in release notes
- Celebrated in our community!

## ❓ Questions?

- Open a [Discussion](https://github.com/your-org/this-rs/discussions)
- Join our community chat
- Email the maintainers

---

**Thank you for contributing to this-rs!** 🚀🦀✨
