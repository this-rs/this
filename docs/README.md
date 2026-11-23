# this-rs Documentation

> **Framework for building complex multi-entity REST and GraphQL APIs with many relationships.**

## 🎯 Is This Framework Right for You?

this-rs is designed for **APIs with 5+ entities and complex relationships**.

**✅ Use this-rs if you have:**
- Many entities (5+) with CRUD operations
- Complex relationships (many-to-many, bidirectional)
- Need for both REST and GraphQL
- Microservices architecture

**⚠️ Consider alternatives if you have:**
- Simple CRUD (< 5 entities)
- No/few relationships
- Learning Rust/Axum (start with Axum directly)

See the main [README](../README.md#is-this-rs-right-for-you) for detailed comparison.

---

## 📚 Navigation Guide

### 🚀 Quick Start

- **[Getting Started](guides/GETTING_STARTED.md)** - First steps with this-rs
- **[Quick Start](guides/QUICK_START.md)** - Quick start guide

### ✅ Validation and Filtering

- **[Validation and Filtering](guides/VALIDATION_AND_FILTERING.md)** - Automatic data validation and filtering
- **[Pagination and Filtering](guides/PAGINATION_AND_FILTERING.md)** - 🆕 Generic pagination and query filtering

### 🔗 Link Management

- **[Enriched Links](guides/ENRICHED_LINKS.md)** - Complete entities in links (auto-enrichment)
- **[Semantic URLs](changes/SEMANTIC_URLS.md)** - 🆕 Coherent and intuitive URLs for links
- **[Link Authorization](guides/LINK_AUTHORIZATION.md)** - Permissions at link level
- **[Link Metadata](guides/LINK_METADATA.md)** - Metadata and link updates
- **[Multi-Level Navigation](guides/MULTI_LEVEL_NAVIGATION.md)** - Multi-level navigation

### 📡 API Exposure

- **[GraphQL](guides/GRAPHQL.md)** - 🆕 Dynamic GraphQL API with automatic schema generation
- **[GraphQL Implementation](architecture/GRAPHQL_IMPLEMENTATION.md)** - Technical details of GraphQL exposure

### 🔄 Comparison & Alternatives

- **[Alternatives](ALTERNATIVES.md)** - 🆕 Honest comparison with other solutions (Axum, async-graphql, SeaORM, etc.)

### 🏗️ Architecture

- **[Architecture](architecture/ARCHITECTURE.md)** - Architecture overview
- **[ServerBuilder Implementation](architecture/SERVER_BUILDER_IMPLEMENTATION.md)** - Auto-generated routes
- **[Routing Explanation](architecture/ROUTING_EXPLANATION.md)** - Routing explanations
- **[GraphQL Implementation](architecture/GRAPHQL_IMPLEMENTATION.md)** - GraphQL exposure architecture

### 📝 Change History

- **[Latest Changes](changes/LATEST_CHANGES.md)** - 🆕 Latest major changes (Pagination & Validation)
- **[Semantic URLs](changes/SEMANTIC_URLS.md)** - Coherent URLs for link operations
- **[Enriched Links Implementation](changes/ENRICHED_LINKS_IMPLEMENTATION.md)** - Enrichment implementation
- **[Auto-Routing Success](changes/AUTO_ROUTING_SUCCESS.md)** - Auto-routing implementation
- **[Module Restructuring](changes/MODULE_RESTRUCTURING.md)** - Module restructuring
- **[Store Simplification](changes/STORE_SIMPLIFICATION.md)** - Store simplification
- **[Entity Folders Structure](changes/ENTITY_FOLDERS_STRUCTURE.md)** - Folder organization

## 🎯 By Use Case

### I want to get started with this-rs
→ [Getting Started](guides/GETTING_STARTED.md)

### I want to understand the architecture
→ [Architecture](architecture/ARCHITECTURE.md)

### I want to create a microservice
→ [Examples README](../examples/microservice/README.md)

### I want to understand auto-routing
→ [ServerBuilder Implementation](architecture/SERVER_BUILDER_IMPLEMENTATION.md)

### I want to understand enriched links
→ [Enriched Links](guides/ENRICHED_LINKS.md)

### I want to manage link permissions
→ [Link Authorization](guides/LINK_AUTHORIZATION.md)

### I want to use GraphQL
→ [GraphQL Guide](guides/GRAPHQL.md)

### I want to compare this-rs with alternatives
→ [Alternatives Comparison](ALTERNATIVES.md)

## 📂 Documentation Structure

```
docs/
├── README.md                    # This file (index)
├── architecture/                # Technical documentation
│   ├── ARCHITECTURE.md
│   ├── SERVER_BUILDER_IMPLEMENTATION.md
│   ├── ROUTING_EXPLANATION.md
│   └── GRAPHQL_IMPLEMENTATION.md  # 🆕 GraphQL technical details
├── guides/                      # User guides
│   ├── GETTING_STARTED.md
│   ├── QUICK_START.md
│   ├── VALIDATION_AND_FILTERING.md  # Data validation and filtering
│   ├── PAGINATION_AND_FILTERING.md  # 🆕 Pagination and query filtering
│   ├── GRAPHQL.md              # 🆕 GraphQL API guide
│   ├── ENRICHED_LINKS.md       # Auto-enrichment of links
│   ├── LINK_AUTHORIZATION.md
│   ├── LINK_METADATA.md
│   └── MULTI_LEVEL_NAVIGATION.md
└── changes/                     # Change history
    ├── SEMANTIC_URLS.md        # 🆕 Semantic URLs for links
    ├── ENRICHED_LINKS_IMPLEMENTATION.md
    ├── AUTO_ROUTING_SUCCESS.md
    ├── LATEST_CHANGES.md
    ├── MODULE_RESTRUCTURING.md
    ├── STORE_SIMPLIFICATION.md
    └── ENTITY_FOLDERS_STRUCTURE.md
```

### Contrats du harnais (`crates/this-harness`)

Ces documents ne décrivent pas une API : ils **qualifient** ce que le harnais
garantit, avec la preuve (`fichier:ligne` ou nom de test) à chaque ligne, et la
liste de ce qui n'est **pas** qualifié. Un chemin sans preuve y est écrit
« non qualifié », jamais présenté comme acquis.

| Document | Question à laquelle il répond | Ticket |
|---|---|---|
| [harnais-mode-auto.md](harnais-mode-auto.md) | Ce que le mode Auto décide, et les lots laissés à l'hôte | — |
| [modules-et-harnais.md](modules-et-harnais.md) | Comment un module exposé devient un outil du harnais | — |
| [harnais-reprise-durable.md](harnais-reprise-durable.md) | Journal best-effort ou reprise durable : qui possède quoi, ce qui manque | #284 |
| [harnais-garanties-runtime.md](harnais-garanties-runtime.md) | Ce qui est **exécuté** en natif/WASM, et ce qui n'est que compilé | #285 |
| [harnais-contrat-evenements.md](harnais-contrat-evenements.md) | Enveloppe d'événements, curseur, attentes humaines | #286 |
| [harnais-autorisations.md](harnais-autorisations.md) | Qui détient chaque autorité, et où le contenu du modèle ne peut pas la produire | #287 |
| [harnais-capacites-moteurs.md](harnais-capacites-moteurs.md) | Matrice par capacité, et « routé » ≠ « servi » | #288 |
| [harnais-contexte-cognitif.md](harnais-contexte-cognitif.md) | Provenance du contexte : ce que le cœur porte, ce que l'hôte doit porter | #290 |
| [harnais-catalogue-outils.md](harnais-catalogue-outils.md) | Un agent qui construit, retrouve, versionne et renforce ses outils — sans les recréer à tout va | — |

## 🔗 Useful Links

- [Main README](../README.md)
- [Examples](../examples/)
- [Source Code](../src/)
