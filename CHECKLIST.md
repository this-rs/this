# ✅ Checklist de Premier Démarrage - This-RS

Utilise cette checklist pour t'assurer que le projet est correctement configuré sur ta machine.

## 🔧 Étape 1: Prérequis

- [ ] Rust est installé (`rustc --version`)
- [ ] Cargo est installé (`cargo --version`)
- [ ] Version Rust >= 1.70 (recommandé)
- [ ] Git est installé (pour version control)
- [ ] Un éditeur de code (VS Code, IntelliJ IDEA, etc.)

### Installation Rust (si nécessaire)
```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
source ~/.cargo/env
```

## 📁 Étape 2: Récupération du Projet

- [ ] Le dossier `this-rs/` est copié sur ta machine
- [ ] Tous les fichiers sont présents (voir liste ci-dessous)

### Fichiers Essentiels
```
✅ Cargo.toml
✅ README.md
✅ GETTING_STARTED.md
✅ TODO.md
✅ Makefile
✅ links.yaml
✅ src/lib.rs
✅ src/core/*.rs (6 fichiers)
✅ src/links/*.rs (3 fichiers)
✅ src/entities/*.rs (2 fichiers)
✅ src/config/*.rs (1 fichier)
✅ examples/simple_api.rs
```

## 🔍 Étape 3: Première Compilation

### 3.1 Vérification Basique
```bash
cd this-rs
make check
# Ou: cargo check
```

**Résultat attendu:**
```
✅ Checking this-rs v0.1.0
✅ Finished dev [unoptimized + debuginfo] target(s)
```

**Si erreur:**
- [ ] Noter le message d'erreur
- [ ] Consulter la section "Erreurs Courantes" ci-dessous

### 3.2 Tests Unitaires
```bash
make test
# Ou: cargo test
```

**Résultat attendu:**
```
running 20+ tests
...
test result: ok. 20 passed; 0 failed
```

### 3.3 Exemple
```bash
make run-example
# Ou: cargo run --example simple_api
```

**Résultat attendu:**
```
🚀 This-RS Simple Example
📋 Creating links...
✅ Created: Alice owns Tesla
...
✨ Example completed successfully!
```

## 🐛 Erreurs Courantes

### Erreur: "cannot find macro `impl_data_entity`"

**Cause:** Problème d'export de macro

**Solution:**
1. Ouvrir `src/entities/macros.rs`
2. Vérifier que la macro est bien marquée `#[macro_export]`
3. Vérifier que `src/lib.rs` réexporte : `pub use entities::macros::*;`

### Erreur: "method `leak` not found"

**Cause:** Utilisation de `.leak()` sur String dynamique dans la macro

**Solution Temporaire:**
Remplacer dans `src/entities/macros.rs`:
```rust
fn resource_name() -> &'static str {
    // ❌ Ne fonctionne pas toujours
    // Pluralizer::pluralize($singular).leak()
    
    // ✅ Solution temporaire
    $singular  // Utiliser directement singular pour l'instant
}
```

### Erreur: "trait bounds were not satisfied"

**Cause:** Problème de trait implementation

**Solution:**
1. Vérifier que toutes les dépendances sont dans `Cargo.toml`
2. Vérifier les versions des dépendances
3. Faire `cargo clean` puis `cargo check`

### Erreur de compilation générale

**Solution rapide:**
```bash
cargo clean
cargo update
cargo check
```

## ✅ Étape 4: Configuration IDE

### VS Code
- [ ] Installer extension "rust-analyzer"
- [ ] Installer extension "CodeLLDB" (pour debugging)
- [ ] Vérifier que l'auto-complétion fonctionne

### IntelliJ IDEA / CLion
- [ ] Installer plugin "Rust"
- [ ] Ouvrir le projet (dossier `this-rs/`)
- [ ] Attendre l'indexation
- [ ] Vérifier que l'auto-complétion fonctionne

## 🎯 Étape 5: Premiers Pas

### 5.1 Comprendre la Structure
- [ ] Lire `README.md` (10 min)
- [ ] Lire `GETTING_STARTED.md` (15 min)
- [ ] Explorer `examples/simple_api.rs` (5 min)

### 5.2 Explorer le Code
- [ ] Ouvrir `src/core/entity.rs` - Comprendre les traits
- [ ] Ouvrir `src/core/link.rs` - Voir la structure Link
- [ ] Ouvrir `src/links/service.rs` - Voir InMemoryLinkService

### 5.3 Modifier l'Exemple
- [ ] Ajouter une nouvelle entité "Company" dans `examples/simple_api.rs`
- [ ] Créer un lien "User works at Company"
- [ ] Vérifier que ça compile et fonctionne

### 5.4 Tests
- [ ] Lancer les tests : `make test`
- [ ] Ajouter un nouveau test dans `src/core/pluralize.rs`
- [ ] Vérifier que ton test passe

## 🚀 Étape 6: Développement

Une fois que tout fonctionne :

- [ ] Consulter `TODO.md` pour voir les tâches prioritaires
- [ ] Choisir une tâche de "Phase 1" (Faire Compiler)
- [ ] Créer une branche Git pour ta feature
- [ ] Implémenter la feature
- [ ] Ajouter des tests
- [ ] Commit et push

### Workflow Recommandé
```bash
# Créer une branche
git checkout -b fix/macro-leak

# Développer avec auto-reload
make watch

# Avant de commit
make all  # Vérifie format + clippy + tests

# Commit
git add .
git commit -m "fix: resolve leak issue in macro"
git push
```

## 📚 Ressources

### Documentation
- [ ] Documentation locale : `make doc`
- [ ] The Rust Book : https://doc.rust-lang.org/book/
- [ ] Axum docs : https://docs.rs/axum/

### Communauté
- [ ] Discord Rust FR (si disponible)
- [ ] Reddit r/rust
- [ ] StackOverflow tag [rust]

## ✅ Validation Finale

Si tu peux faire tout ça, tu es prêt :

```bash
# Tout devrait passer sans erreur
make check     # ✅
make test      # ✅
make clippy    # ✅
make fmt       # ✅
make run-example # ✅
```

## 🎉 Félicitations !

Si tous les checks sont verts, tu es prêt à développer sur This-RS !

**Prochaines étapes suggérées :**
1. Lire `GETTING_STARTED.md` en détail
2. Consulter `TODO.md` Phase 1
3. Commencer par corriger les bugs de compilation si nécessaire
4. Implémenter une feature simple (ex: améliorer les tests)

---

**Besoin d'aide ?**
- Consulte `GETTING_STARTED.md`
- Regarde les exemples dans `examples/`
- Lis la documentation inline dans le code
- Crée une issue sur GitHub (quand créé)

**Bon courage ! 🚀**
