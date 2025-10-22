# This-RS Makefile
# Commandes utiles pour le développement

.PHONY: help check test build run-example doc fmt clippy clean coverage

# Commande par défaut
help:
	@echo "📦 This-RS Framework - Commandes Disponibles"
	@echo ""
	@echo "  make check          - Vérifier que le code compile"
	@echo "  make test           - Lancer tous les tests"
	@echo "  make test-verbose   - Tests avec output détaillé"
	@echo "  make build          - Compiler le projet"
	@echo "  make run-example    - Lancer l'exemple simple"
	@echo "  make doc            - Générer et ouvrir la documentation"
	@echo "  make fmt            - Formater le code"
	@echo "  make clippy         - Linter le code"
	@echo "  make clean          - Nettoyer les artifacts"
	@echo "  make coverage       - Générer rapport de couverture"
	@echo "  make all            - check + test + clippy + fmt"
	@echo ""

# Vérifier la compilation
check:
	@echo "🔍 Vérification de la compilation..."
	cargo check --all-features

# Lancer les tests
test:
	@echo "🧪 Lancement des tests..."
	cargo test

# Tests avec output
test-verbose:
	@echo "🧪 Tests avec output détaillé..."
	cargo test -- --nocapture --test-threads=1

# Compiler en mode debug
build:
	@echo "🔨 Compilation..."
	cargo build

# Compiler en mode release
build-release:
	@echo "🔨 Compilation optimisée..."
	cargo build --release

# Lancer l'exemple
run-example:
	@echo "🚀 Lancement de l'exemple..."
	cargo run --example simple_api

# Générer la documentation
doc:
	@echo "📚 Génération de la documentation..."
	cargo doc --open --no-deps

# Formater le code
fmt:
	@echo "✨ Formatage du code..."
	cargo fmt

# Vérifier le formatage
fmt-check:
	@echo "✨ Vérification du formatage..."
	cargo fmt --check

# Linter
clippy:
	@echo "🔍 Analyse du code avec Clippy..."
	cargo clippy -- -D warnings

# Nettoyer
clean:
	@echo "🧹 Nettoyage..."
	cargo clean

# Coverage (nécessite cargo-tarpaulin)
coverage:
	@echo "📊 Génération du rapport de couverture..."
	@if command -v cargo-tarpaulin >/dev/null 2>&1; then \
		cargo tarpaulin --out Html --output-dir target/coverage; \
		echo "Rapport généré dans target/coverage/index.html"; \
	else \
		echo "❌ cargo-tarpaulin non installé. Installer avec:"; \
		echo "   cargo install cargo-tarpaulin"; \
	fi

# Tout vérifier
all: fmt check clippy test
	@echo "✅ Toutes les vérifications sont passées !"

# Installation des outils de développement
install-tools:
	@echo "🔧 Installation des outils..."
	cargo install cargo-tarpaulin
	cargo install cargo-watch
	cargo install cargo-expand
	@echo "✅ Outils installés !"

# Watch mode (nécessite cargo-watch)
watch:
	@echo "👀 Mode watch activé..."
	cargo watch -x check -x test

# Benchmarks (quand implémentés)
bench:
	@echo "⚡ Lancement des benchmarks..."
	cargo bench

# Afficher l'arborescence du projet
tree:
	@./tree.sh

# Initialiser un nouveau projet utilisateur
init-project:
	@echo "🎯 Création d'un nouveau projet avec This-RS..."
	@echo "Cette fonctionnalité sera implémentée dans le CLI"
	@echo "Pour l'instant, copie le template manuellement"

# Publier sur crates.io (avec vérifications)
publish-check:
	@echo "🔍 Vérification avant publication..."
	cargo publish --dry-run

publish:
	@echo "📦 Publication sur crates.io..."
	@echo "⚠️  Êtes-vous sûr ? (Ctrl+C pour annuler)"
	@read -p "Appuyez sur Entrée pour continuer..."
	cargo publish
