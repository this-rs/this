# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- GitHub Actions CI/CD workflows
  - Continuous Integration with multi-platform testing
  - Automated releases to crates.io
  - Documentation deployment to GitHub Pages
  - Security audits with cargo-audit
- Contribution guidelines and templates
- Pull request and issue templates
- **Multi-module configuration merging**
  - `LinksConfig::merge()` method for intelligent config merging
  - Support for multiple modules with automatic configuration merge
  - Entities: last definition wins for duplicates
  - Links: last definition wins for duplicates
  - Validation rules: combined for same link_type
  - Comprehensive tests for all merge scenarios
  - Documentation guide for multi-module setup

### Changed
- `ServerBuilder::merge_configs()` now uses proper config merging instead of just taking first config

### Fixed
- **Config merging TODO**: Implemented proper multi-module configuration merging (was marked as TODO)

### Removed

## [0.1.0] - 2025-10-22

### Added
- Initial release
- Generic entity and relationship management framework
- Auto-generated CRUD routes for entities
- Bidirectional link navigation
- Multi-tenant support with tenant isolation
- YAML-based configuration for links
- Auto-pluralization system
- Flexible relationships between entities
- In-memory storage implementation
- ServerBuilder for fluent API
- EntityRegistry for auto-route generation
- Link enrichment with full entity data
- Contextual link fetching (forward/reverse/direct)
- Three example applications:
  - simple_api: Basic CRUD
  - full_api: Complete API with all features
  - microservice: Billing microservice example

### Core Components
- `EntityDescriptor` trait for route generation
- `Module` trait for grouping entities
- `LinkService` for relationship management
- `EntityFetcher` for dynamic entity loading
- `EntityRegistry` for collecting and building routes

### Documentation
- Comprehensive README with examples
- Architecture documentation
- Getting started guide
- API documentation
- Link enrichment guide
- Multi-level navigation guide

[Unreleased]: https://github.com/triviere/this-rs/compare/v0.1.0...HEAD
[0.1.0]: https://github.com/triviere/this-rs/releases/tag/v0.1.0
