# GitHub Actions Workflows

Ce dossier contient les workflows CI/CD pour This-RS.

## 📋 Workflows

### 🧪 CI (Continuous Integration)
**Fichier:** `workflows/ci.yml`

Exécuté sur chaque push et pull request vers `main` et `develop`.

**Jobs:**
- **Test Suite**: Tests sur Rust stable, beta, et nightly
- **Rustfmt**: Vérification du formatage du code
- **Clippy**: Linting avec clippy (warnings = errors)
- **Examples**: Compilation de tous les exemples
- **Security Audit**: Audit de sécurité avec cargo-audit
- **Documentation**: Vérification de la doc (warnings = errors)
- **Cross Platform**: Tests sur Linux, Windows, et macOS
- **Minimal Versions**: Vérification des versions minimales de dépendances

### 📦 Release
**Fichier:** `workflows/release.yml`

Exécuté lors de la création d'un tag `v*.*.*` (ex: `v0.1.0`).

**Jobs:**
- **Create GitHub Release**: Crée une release GitHub
- **Publish to crates.io**: Publie la crate sur crates.io
- **Build Binaries**: Build des binaries multi-plateformes (optionnel)

**Pour créer une release:**
```bash
git tag v0.1.0
git push origin v0.1.0
```

### 📚 Documentation
**Fichier:** `workflows/docs.yml`

Exécuté sur chaque push vers `main`.

**Jobs:**
- **Build Documentation**: Génère la documentation rustdoc
- **Deploy to GitHub Pages**: Déploie sur GitHub Pages

**Configuration requise:**
1. Aller dans Settings > Pages
2. Source: GitHub Actions
3. La doc sera disponible à: `https://<username>.github.io/<repo>/`

### 🔄 Dependabot
**Fichier:** `dependabot.yml`

Mises à jour automatiques hebdomadaires (lundis à 9h) pour:
- Dépendances Cargo
- GitHub Actions

## 🔑 Secrets Requis

Pour que tous les workflows fonctionnent, configurez ces secrets dans Settings > Secrets and variables > Actions:

- `CARGO_TOKEN`: Token pour publier sur crates.io
  - Créer sur https://crates.io/settings/tokens
  - Permissions: `publish-update`

- `GITHUB_TOKEN`: Fourni automatiquement par GitHub ✅

## 🚀 Quick Start

1. **Fork/Clone le repo**
2. **Configurer les secrets** (voir ci-dessus)
3. **Push du code** → CI se lance automatiquement
4. **Créer un tag** → Release se lance automatiquement
5. **Activer GitHub Pages** → Documentation en ligne

## 📊 Badges

Ajoutez ces badges dans votre README principal:

```markdown
[![CI](https://github.com/USERNAME/this-rs/actions/workflows/ci.yml/badge.svg)](https://github.com/USERNAME/this-rs/actions/workflows/ci.yml)
[![Documentation](https://github.com/USERNAME/this-rs/actions/workflows/docs.yml/badge.svg)](https://github.com/USERNAME/this-rs/actions/workflows/docs.yml)
[![Crates.io](https://img.shields.io/crates/v/this-rs.svg)](https://crates.io/crates/this-rs)
[![License](https://img.shields.io/crates/l/this-rs.svg)](LICENSE-MIT)
```

## 🛠️ Personnalisation

### Modifier les branches surveillées
Dans `ci.yml`:
```yaml
on:
  push:
    branches: [ main, develop, feature/* ]  # Ajoutez vos branches
```

### Désactiver certains jobs
Commentez ou supprimez les jobs non nécessaires.

### Changer la fréquence Dependabot
Dans `dependabot.yml`:
```yaml
schedule:
  interval: "daily"  # ou "monthly"
```

## 📝 Notes

- **Cache**: Les workflows utilisent le cache Cargo pour accélérer les builds
- **Fail Fast**: Les tests continuent même si une version de Rust échoue
- **Security**: cargo-audit peut échouer en `continue-on-error` pour ne pas bloquer la CI
- **Minimal Versions**: Nécessite Rust nightly, peut être désactivé si problématique
