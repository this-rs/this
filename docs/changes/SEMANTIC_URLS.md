# URLs Sémantiques pour les Liens

## 🎯 Objectif

Rendre les URLs des opérations de liens **cohérentes** et **sémantiques** en utilisant le `route_name` au lieu du `link_type` technique.

## 📊 Avant vs Après

### ❌ Ancien Format (Incohérent)

```bash
# Liste (utilisait route_name)
GET /users/123/cars-owned

# Création (utilisait link_type) ← INCOHÉRENT
POST /users/123/owner/cars/456

# Mise à jour (utilisait link_type) ← INCOHÉRENT
PUT /users/123/owner/cars/456

# Suppression (utilisait link_type) ← INCOHÉRENT
DELETE /users/123/owner/cars/456
```

**Problèmes** :
- ❌ Incohérence entre listing et manipulation
- ❌ URLs moins intuitives (`owner` vs `cars-owned`)
- ❌ Nécessite de connaître le `link_type` technique
- ❌ Format d'URL plus long (5 segments au lieu de 4)

### ✅ Nouveau Format (Cohérent)

```bash
# Liste
GET /users/123/cars-owned

# Création ← COHÉRENT
POST /users/123/cars-owned/456

# Mise à jour ← COHÉRENT
PUT /users/123/cars-owned/456

# Suppression ← COHÉRENT
DELETE /users/123/cars-owned/456
```

**Avantages** :
- ✅ **Cohérence totale** : toutes les opérations utilisent le même pattern
- ✅ **URLs sémantiques** : `cars-owned` est auto-documenté
- ✅ **Plus court** : 4 segments au lieu de 5
- ✅ **RESTful** : suit les conventions REST
- ✅ **Pas de conflit** : résout naturellement les relations multiples

## 🔄 Résolution des Relations Multiples

### Configuration YAML

```yaml
links:
  # User possède une voiture
  - link_type: owner
    source_type: user
    target_type: car
    forward_route_name: cars-owned      # ← Route unique
    reverse_route_name: users-owners
  
  # User conduit une voiture (relation différente)
  - link_type: driver
    source_type: user
    target_type: car
    forward_route_name: cars-driven     # ← Route unique
    reverse_route_name: users-drivers
```

### URLs Générées (Sans Conflit)

```bash
# Relation "owner"
GET    /users/123/cars-owned           # Liste les voitures possédées
POST   /users/123/cars-owned/456       # Crée un lien de possession
PUT    /users/123/cars-owned/456       # Met à jour le lien
DELETE /users/123/cars-owned/456       # Supprime le lien

# Relation "driver" (même entités, route différente)
GET    /users/123/cars-driven          # Liste les voitures conduites
POST   /users/123/cars-driven/456      # Crée un lien de conduite
PUT    /users/123/cars-driven/456      # Met à jour le lien
DELETE /users/123/cars-driven/456      # Supprime le lien
```

**Résultat** : Pas de conflit possible car chaque relation a son propre `route_name` unique !

## 🏗️ Architecture Technique

### 1. DirectLinkExtractor

Le `DirectLinkExtractor` a été modifié pour accepter `route_name` au lieu de `link_type` :

```rust
// Ancien format
pub fn from_path(
    path_parts: (String, Uuid, String, String, Uuid),  // link_type + target_type
    config: &LinksConfig,
    tenant_id: Uuid,
) -> Result<Self, ExtractorError>

// Nouveau format
pub fn from_path(
    path_parts: (String, Uuid, String, Uuid),  // route_name seulement
    registry: &LinkRouteRegistry,
    config: &LinksConfig,
    tenant_id: Uuid,
) -> Result<Self, ExtractorError>
```

### 2. Résolution Automatique

Le `LinkRouteRegistry` résout automatiquement le `route_name` vers le `link_definition` :

```rust
let (link_definition, direction) = registry
    .resolve_route(&source_type, &route_name)
    .map_err(|_| ExtractorError::RouteNotFound(route_name.clone()))?;

// Le link_type est extrait de la définition
let link_type = &link_definition.link_type;
```

### 3. Handlers Mis à Jour

Tous les handlers utilisent maintenant le nouveau format :

```rust
// create_link
Path((source_type_plural, source_id, route_name, target_id)): Path<(
    String,
    Uuid,
    String,  // route_name au lieu de link_type
    Uuid,    // pas de target_type
)>

// update_link (même signature)
// delete_link (même signature)
```

## 📝 Exemples Complets

### Exemple 1 : Order → Invoice

```bash
# Configuration
link_type: has_invoice
forward_route_name: invoices
reverse_route_name: order

# URLs Forward (depuis order)
GET    /orders/abc-123/invoices           # Liste
POST   /orders/abc-123/invoices/def-456   # Crée
PUT    /orders/abc-123/invoices/def-456   # Met à jour
DELETE /orders/abc-123/invoices/def-456   # Supprime

# URLs Reverse (depuis invoice)
GET    /invoices/def-456/order            # Liste (1 seul)
POST   /invoices/def-456/order/abc-123    # Crée (même lien)
DELETE /invoices/def-456/order/abc-123    # Supprime (même lien)
```

### Exemple 2 : User ↔ Company (Relations Multiples)

```bash
# Configuration
# Relation 1: owner
link_type: owner
forward_route_name: companies-owned
reverse_route_name: users-owners

# Relation 2: worker
link_type: worker
forward_route_name: companies-work
reverse_route_name: users-workers

# URLs pour "owner"
GET    /users/123/companies-owned         # Companies possédées
POST   /users/123/companies-owned/456     # Crée possession
PUT    /users/123/companies-owned/456     # Met à jour
DELETE /users/123/companies-owned/456     # Supprime

# URLs pour "worker"
GET    /users/123/companies-work          # Companies où je travaille
POST   /users/123/companies-work/456      # Crée emploi
PUT    /users/123/companies-work/456      # Met à jour (ex: rôle)
DELETE /users/123/companies-work/456      # Supprime emploi

# Reverse depuis company
GET    /companies/456/users-owners        # Propriétaires
GET    /companies/456/users-workers       # Employés
```

## 🧪 Tests Validés

Tous les tests ont été validés avec le nouveau format :

```bash
# Test 1: Liste des liens
✅ GET /orders/{id}/invoices
   → Retourne les invoices avec enrichissement

# Test 2: Création de lien
✅ POST /orders/{id}/invoices/{invoice_id}
   → Crée le lien avec metadata

# Test 3: Mise à jour de lien
✅ PUT /orders/{id}/invoices/{invoice_id}
   → Met à jour la metadata

# Test 4: Suppression de lien
✅ DELETE /orders/{id}/invoices/{invoice_id}
   → Supprime le lien (204 No Content)
```

## 🎯 Impact sur les Utilisateurs

### Migration Nécessaire

Si vous utilisez l'ancien format, vous devez mettre à jour vos URLs :

```bash
# Ancien
POST /users/123/owner/cars/456
PUT  /users/123/owner/cars/456
DELETE /users/123/owner/cars/456

# Nouveau
POST /users/123/cars-owned/456
PUT  /users/123/cars-owned/456
DELETE /users/123/cars-owned/456
```

### Compatibilité

- ✅ Les routes de **listing** (`GET /{entity}/{id}/{route_name}`) n'ont pas changé
- ✅ Les routes d'**introspection** (`GET /{entity}/{id}/links`) n'ont pas changé
- ✅ Les routes de **get par ID** (`GET /links/{link_id}`) n'ont pas changé
- ⚠️ Les routes de **création/mise à jour/suppression** ont changé de format

## 📚 Documentation Mise à Jour

Les documents suivants ont été mis à jour :

- ✅ `src/core/extractors.rs` - Commentaires et signatures
- ✅ `src/links/handlers.rs` - Commentaires des handlers
- ✅ `src/server/router.rs` - Documentation des routes
- ✅ `examples/microservice/main.rs` - Exemples de curl
- ✅ `docs/guides/ENRICHED_LINKS.md` - Guide des liens enrichis
- ✅ `docs/changes/SEMANTIC_URLS.md` - Ce document

## 🎉 Conclusion

Ce changement apporte :

1. **Cohérence** : Toutes les opérations utilisent le même pattern d'URL
2. **Sémantique** : Les URLs sont auto-documentées et intuitives
3. **Simplicité** : URLs plus courtes (4 segments au lieu de 5)
4. **Robustesse** : Résolution automatique des relations multiples
5. **RESTful** : Suit les meilleures pratiques REST

Le framework `this-rs` est maintenant encore plus élégant et facile à utiliser ! 🚀🦀✨

