# GitHub Actions Workflows

Ce dossier contient les workflows CI/CD pour This-RS.

## 📋 Workflows Disponibles

### 🧪 [ci.yml](workflows/ci.yml)
**Continuous Integration** - Exécuté sur chaque push et PR vers `main`/`develop`

**Jobs inclus:**
- Tests (stable, beta, nightly)
- Rustfmt (formatage)
- Clippy (linting)
- Examples (compilation)
- Security audit
- Documentation
- Cross-platform (Linux, Windows, macOS)
- Minimal versions

### 📦 [release.yml](workflows/release.yml)
**Release Automation** - Exécuté sur les tags `v*.*.*`

**Jobs inclus:**
- Création de GitHub Release
- Publication sur crates.io
- Build de binaires multi-plateformes

**Usage:**
```bash
git tag v0.1.0
git push origin v0.1.0
```

### 📚 [docs.yml](workflows/docs.yml)
**Documentation Deployment** - Exécuté sur chaque push vers `main`

**Jobs inclus:**
- Build de la documentation rustdoc
- Déploiement sur GitHub Pages

**Configuration requise:**
- Settings > Pages > Source: GitHub Actions

### 🔄 [dependabot.yml](dependabot.yml)
**Dependency Updates** - Mises à jour automatiques hebdomadaires

- Dépendances Cargo
- GitHub Actions

## 🔑 Secrets Requis

Configure dans **Settings > Secrets and variables > Actions:**

- `CARGO_TOKEN`: Token crates.io (https://crates.io/settings/tokens)
- `GITHUB_TOKEN`: ✅ Fourni automatiquement

## 📊 Status Badges

```markdown
[![CI](https://github.com/this-rs/this/actions/workflows/ci.yml/badge.svg)](https://github.com/this-rs/this/actions/workflows/ci.yml)
[![Documentation](https://github.com/this-rs/this/actions/workflows/docs.yml/badge.svg)](https://github.com/this-rs/this/actions/workflows/docs.yml)
```

## 🛠️ Personnalisation

Pour modifier les workflows, éditez les fichiers dans `workflows/`.

Pour plus de détails, consultez [CONTRIBUTING.md](../CONTRIBUTING.md).
