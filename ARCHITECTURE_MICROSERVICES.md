# Architecture Microservices - This-RS

## 🎯 Vision et Objectifs

Le framework `this-rs` est conçu comme un **core minimaliste** pour construire des microservices qui :

1. **Exposent automatiquement** des endpoints CRUD pour leurs entités
2. **Gèrent les relations** entre entités via un système de liens bidirectionnel
3. **Isolent les tenants** nativement via `tenant_id`
4. **Contrôlent l'accès** via un système d'autorisation granulaire
5. **S'intègrent** avec ScyllaDB (données) et Neo4j (liens, optionnel)

## 🏗️ Architecture en Couches

```
┌─────────────────────────────────────────────────────────────┐
│                    Microservice Client                        │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐       │
│  │   Order      │  │   Invoice    │  │   Payment    │       │
│  │   Entity     │  │   Entity     │  │   Entity     │       │
│  └──────────────┘  └──────────────┘  └──────────────┘       │
│                                                               │
│  ┌──────────────────────────────────────────────────────┐   │
│  │          Module Implementation                        │   │
│  │  - entity_types()                                     │   │
│  │  - links_config()                                     │   │
│  └──────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                            │
                            │ uses
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                    This-RS Core Framework                     │
│                                                               │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐       │
│  │    Auth      │  │   Entities   │  │    Links     │       │
│  │   System     │  │   System     │  │   System     │       │
│  └──────────────┘  └──────────────┘  └──────────────┘       │
│                                                               │
│  ┌──────────────────────────────────────────────────────┐   │
│  │            Configuration (YAML)                       │   │
│  │  - Entity definitions + auth policies                 │   │
│  │  - Link definitions + validation rules                │   │
│  └──────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
                            │
                            │ stores in
                            ▼
┌─────────────────────────────────────────────────────────────┐
│                     Storage Layer                             │
│                                                               │
│  ┌──────────────────┐         ┌──────────────────┐          │
│  │   ScyllaDB       │         │     Neo4j        │          │
│  │   (Entities)     │         │    (Links)       │          │
│  └──────────────────┘         └──────────────────┘          │
└─────────────────────────────────────────────────────────────┘
```

## 🧩 Composants Core

### 1. Module System (`core/module.rs`)

Le trait `Module` définit l'interface pour un microservice :

```rust
pub trait Module: Send + Sync {
    fn name(&self) -> &str;
    fn version(&self) -> &str;
    fn entity_types(&self) -> Vec<&str>;
    fn links_config(&self) -> Result<LinksConfig>;
}
```

**Avantages** :
- Découverte automatique des entités
- Chargement de configuration isolé
- Versioning du microservice

### 2. Auth System (`core/auth.rs`)

#### AuthContext

Représente le contexte d'authentification d'une requête :

```rust
pub enum AuthContext {
    User { user_id, tenant_id, roles },    // User authentifié
    Owner { user_id, resource_id, ... },   // Propriétaire
    Service { service_name, ... },         // Service-to-service
    Admin { admin_id },                    // Admin
    Anonymous,                             // Public
}
```

#### AuthPolicy

Définit les règles d'autorisation :

```rust
pub enum AuthPolicy {
    Public,                      // Accès public
    Authenticated,               // User authentifié
    Owner,                       // Propriétaire de la ressource
    HasRole(Vec<String>),        // Roles requis
    ServiceOnly,                 // Service-to-service
    AdminOnly,                   // Admin uniquement
    And(Vec<AuthPolicy>),        // Combinaison ET
    Or(Vec<AuthPolicy>),         // Combinaison OU
    Custom(fn(&AuthContext) -> bool), // Custom
}
```

#### AuthProvider

Trait pour implémenter l'extraction et vérification d'auth :

```rust
#[async_trait]
pub trait AuthProvider: Send + Sync {
    async fn extract_context<B>(&self, req: &Request<B>) -> Result<AuthContext>;
    async fn is_owner(&self, user_id: &Uuid, resource_id: &Uuid, resource_type: &str) -> Result<bool>;
    async fn has_role(&self, user_id: &Uuid, role: &str) -> Result<bool>;
}
```

### 3. Entity System (`core/entity.rs`)

Traits pour définir des entités génériques :

```rust
pub trait Entity: Sized + Send + Sync + 'static {
    type Service: Send + Sync;
    fn resource_name() -> &'static str;
    fn resource_name_singular() -> &'static str;
}

pub trait Data: Entity {
    fn id(&self) -> Uuid;
    fn tenant_id(&self) -> Uuid;
    fn indexed_fields() -> &'static [&'static str];
    fn field_value(&self, field: &str) -> Option<FieldValue>;
    fn type_name() -> &'static str;
}
```

### 4. Link System (`core/link.rs`)

Gestion des relations polymorphes :

```rust
pub struct EntityReference {
    pub id: Uuid,
    pub entity_type: String,  // Polymorphe !
}

pub struct Link {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub link_type: String,    // Polymorphe !
    pub source: EntityReference,
    pub target: EntityReference,
    pub metadata: Option<Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

### 5. Configuration (`config/mod.rs`)

#### EntityConfig

Configuration d'une entité avec auth :

```yaml
entities:
  - singular: order
    plural: orders
    auth:
      list: authenticated          # GET /orders
      get: authenticated           # GET /orders/{id}
      create: authenticated        # POST /orders
      update: owner                # PUT /orders/{id}
      delete: owner_or_role:admin  # DELETE /orders/{id}
      list_links: authenticated    # GET /orders/{id}/invoices
      create_link: owner           # POST /orders/{id}/has_invoice/...
      delete_link: owner           # DELETE /orders/{id}/has_invoice/...
```

#### LinkDefinition

Définition d'une relation :

```yaml
links:
  - link_type: has_invoice
    source_type: order
    target_type: invoice
    forward_route_name: invoices      # /orders/{id}/invoices
    reverse_route_name: order         # /invoices/{id}/order
    description: "Order has invoices"
```

## 🔄 Flux de Requête

### Exemple : GET /orders/123/invoices

```
1. Client envoie requête HTTP
   ↓
2. Axum router capture la route
   ↓
3. LinkExtractor parse le path
   - entity_type = "order"
   - entity_id = "123"
   - route_name = "invoices"
   ↓
4. LinkRouteRegistry résout la route
   - Trouve LinkDefinition (forward)
   - link_type = "has_invoice"
   - target_type = "invoice"
   ↓
5. AuthProvider extrait AuthContext
   - Vérifie JWT / Headers
   - Détermine user_id, tenant_id, roles
   ↓
6. AuthPolicy.check(context)
   - Vérifie policy de l'entité "order"
   - list_links: "authenticated" → OK
   ↓
7. LinkService.get_by_source()
   - Query: source={id:123, type:"order"}, link_type:"has_invoice"
   - Filtre par tenant_id
   ↓
8. Retourne liste de Links
   ↓
9. Handler enrichit avec détails si besoin
   ↓
10. Retourne JSON au client
```

## 🗃️ Stratégies de Stockage

### Option 1 : ScyllaDB pour Tout

**Tables** :
```sql
-- Table principale des entités (par type)
CREATE TABLE orders (
    tenant_id uuid,
    id uuid,
    order_number text,
    total_amount decimal,
    status text,
    created_at timestamp,
    updated_at timestamp,
    PRIMARY KEY ((tenant_id), id)
);

-- Table des liens
CREATE TABLE links (
    tenant_id uuid,
    id uuid,
    link_type text,
    source_id uuid,
    source_type text,
    target_id uuid,
    target_type text,
    metadata text,  -- JSON
    created_at timestamp,
    updated_at timestamp,
    PRIMARY KEY ((tenant_id), id)
);

-- Index pour recherche par source
CREATE MATERIALIZED VIEW links_by_source AS
    SELECT * FROM links
    WHERE tenant_id IS NOT NULL AND source_id IS NOT NULL
    PRIMARY KEY ((tenant_id, source_id), link_type, id);

-- Index pour recherche par target (reverse)
CREATE MATERIALIZED VIEW links_by_target AS
    SELECT * FROM links
    WHERE tenant_id IS NOT NULL AND target_id IS NOT NULL
    PRIMARY KEY ((tenant_id, target_id), link_type, id);
```

**Avantages** :
- ✅ Latence ultra-basse
- ✅ Scalabilité horizontale
- ✅ Simplicité (une seule technologie)

**Inconvénients** :
- ❌ Pas de requêtes graph complexes (shortest path, etc.)
- ❌ Duplication de données (MVs)

### Option 2 : ScyllaDB (Entities) + Neo4j (Links)

**ScyllaDB** : Stockage des entités
**Neo4j** : Stockage des liens comme graph

```cypher
// Créer un lien dans Neo4j
CREATE (o:Order {id: '123', tenant_id: 'abc'})
CREATE (i:Invoice {id: '456', tenant_id: 'abc'})
CREATE (o)-[:HAS_INVOICE {
    link_id: 'xyz',
    created_at: datetime()
}]->(i)

// Recherche bidirectionnelle
MATCH (o:Order {id: '123'})-[:HAS_INVOICE]->(i:Invoice)
WHERE o.tenant_id = 'abc'
RETURN i

// Recherche inverse
MATCH (o:Order)-[:HAS_INVOICE]->(i:Invoice {id: '456'})
WHERE i.tenant_id = 'abc'
RETURN o
```

**Avantages** :
- ✅ Requêtes graph complexes possibles
- ✅ Visualisation des relations
- ✅ Pas de duplication pour bi-directionalité

**Inconvénients** :
- ❌ Deux systèmes à gérer
- ❌ Latence potentiellement plus élevée
- ❌ Complexité opérationnelle

### Recommandation

**Phase 1** : Commencer avec **ScyllaDB uniquement**
- Implémentation plus simple
- Moins de complexité opérationnelle
- Suffisant pour 95% des cas d'usage

**Phase 2** : Ajouter **Neo4j si besoin**
- Seulement si requêtes graph complexes nécessaires
- Peut co-exister avec ScyllaDB
- Migration progressive

## 🚀 Implémentation ScyllaDB

### 1. Créer `ScyllaDBLinkService`

```rust
pub struct ScyllaDBLinkService {
    session: Arc<Session>,
}

#[async_trait]
impl LinkService for ScyllaDBLinkService {
    async fn create(&self, tenant_id: &Uuid, link_type: &str, 
                    source: EntityReference, target: EntityReference,
                    metadata: Option<Value>) -> Result<Link> {
        let link = Link {
            id: Uuid::new_v4(),
            tenant_id: *tenant_id,
            link_type: link_type.to_string(),
            source,
            target,
            metadata,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        
        let query = "INSERT INTO links (tenant_id, id, link_type, source_id, source_type, target_id, target_type, metadata, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)";
        
        self.session.query(query, (
            &link.tenant_id,
            &link.id,
            &link.link_type,
            &link.source.id,
            &link.source.entity_type,
            &link.target.id,
            &link.target.entity_type,
            serde_json::to_string(&link.metadata)?,
            &link.created_at,
            &link.updated_at,
        )).await?;
        
        Ok(link)
    }
    
    async fn get_by_source(&self, tenant_id: &Uuid, source: &EntityReference, 
                          link_type: Option<&str>) -> Result<Vec<Link>> {
        let query = if let Some(lt) = link_type {
            "SELECT * FROM links_by_source WHERE tenant_id = ? AND source_id = ? AND link_type = ?"
        } else {
            "SELECT * FROM links_by_source WHERE tenant_id = ? AND source_id = ?"
        };
        
        // Execute query and map rows to Link structs
        // ...
    }
    
    // Implémenter les autres méthodes...
}
```

### 2. Initialiser la connexion

```rust
#[tokio::main]
async fn main() -> Result<()> {
    // Connecter à ScyllaDB
    let session = SessionBuilder::new()
        .known_node("127.0.0.1:9042")
        .build()
        .await?;
    
    let session = Arc::new(session);
    
    // Créer le service
    let link_service = Arc::new(ScyllaDBLinkService::new(session));
    
    // Utiliser dans AppState
    let app_state = AppState {
        link_service,
        registry,
        config,
    };
    
    // Setup routes et start server
    // ...
}
```

## 🔐 Implémentation Auth

### 1. Créer `JwtAuthProvider`

```rust
pub struct JwtAuthProvider {
    secret: Vec<u8>,
    scylla: Arc<Session>,
}

#[async_trait]
impl AuthProvider for JwtAuthProvider {
    async fn extract_context<B>(&self, req: &Request<B>) -> Result<AuthContext> {
        // 1. Extraire JWT du header Authorization
        let auth_header = req.headers()
            .get("Authorization")
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| anyhow!("Missing Authorization header"))?;
        
        let token = auth_header
            .strip_prefix("Bearer ")
            .ok_or_else(|| anyhow!("Invalid Authorization format"))?;
        
        // 2. Valider et décoder JWT
        let claims = decode_jwt(token, &self.secret)?;
        
        // 3. Créer AuthContext
        Ok(AuthContext::User {
            user_id: claims.user_id,
            tenant_id: claims.tenant_id,
            roles: claims.roles,
        })
    }
    
    async fn is_owner(&self, user_id: &Uuid, resource_id: &Uuid, 
                     resource_type: &str) -> Result<bool> {
        // Query ScyllaDB pour vérifier ownership
        let query = format!(
            "SELECT id FROM {} WHERE tenant_id = ? AND id = ? AND owner_id = ?",
            resource_type
        );
        
        let result = self.scylla.query(query, (tenant_id, resource_id, user_id)).await?;
        Ok(result.rows.is_some())
    }
    
    async fn has_role(&self, user_id: &Uuid, role: &str) -> Result<bool> {
        // Query pour vérifier role
        // ...
    }
}
```

### 2. Intégrer Auth dans Handlers

```rust
pub async fn list_links(
    State(state): State<AppState>,
    auth: Extension<AuthContext>,  // Injecté par middleware
    Path((entity_type, entity_id, route_name)): Path<(String, String, String)>,
) -> Result<Json<Value>, ExtractorError> {
    // 1. Résoudre la route
    let (link_def, direction) = state.registry.resolve_route(&entity_type, &route_name)?;
    
    // 2. Vérifier auth policy
    let entity_config = state.config.entities
        .iter()
        .find(|e| e.singular == entity_type)
        .ok_or_else(|| anyhow!("Entity not found"))?;
    
    let policy = AuthPolicy::from_str(&entity_config.auth.list_links);
    
    if !policy.check(&auth) {
        return Err(ExtractorError::Unauthorized);
    }
    
    // 3. Continuer avec la logique...
}
```

## 📈 Scalabilité et Performance

### Multi-tenancy

- Toutes les queries incluent `tenant_id` dans la partition key
- Isolation stricte au niveau DB
- Pas de data leakage possible

### Caching

```rust
pub struct CachedLinkService {
    inner: Arc<dyn LinkService>,
    cache: Arc<RwLock<LruCache<String, Vec<Link>>>>,
}

#[async_trait]
impl LinkService for CachedLinkService {
    async fn get_by_source(&self, tenant_id: &Uuid, source: &EntityReference,
                          link_type: Option<&str>) -> Result<Vec<Link>> {
        let cache_key = format!("{}-{}-{:?}", tenant_id, source.id, link_type);
        
        // Check cache
        if let Some(links) = self.cache.read().await.get(&cache_key) {
            return Ok(links.clone());
        }
        
        // Cache miss - query DB
        let links = self.inner.get_by_source(tenant_id, source, link_type).await?;
        
        // Update cache
        self.cache.write().await.put(cache_key, links.clone());
        
        Ok(links)
    }
}
```

### Pagination

```rust
pub async fn list_links_paginated(
    State(state): State<AppState>,
    Query(params): Query<PaginationParams>,
    // ...
) -> Result<Json<PaginatedResponse<Link>>> {
    let links = state.link_service
        .get_by_source_paginated(
            &tenant_id,
            &source,
            link_type,
            params.page,
            params.page_size,
        )
        .await?;
    
    Ok(Json(PaginatedResponse {
        data: links,
        page: params.page,
        page_size: params.page_size,
        total: total_count,
    }))
}
```

## ✅ Checklist Migration vers Production

### Phase 1 : Foundation
- [ ] Implémenter `ScyllaDBLinkService`
- [ ] Créer schémas Scylla + MVs
- [ ] Tests d'intégration avec Scylla
- [ ] Implémenter `JwtAuthProvider`
- [ ] Intégrer auth dans handlers

### Phase 2 : Robustesse
- [ ] Ajouter retry logic (exponential backoff)
- [ ] Implémenter circuit breaker
- [ ] Ajouter caching (Redis ou in-memory LRU)
- [ ] Logging structuré (tracing)
- [ ] Métriques (Prometheus)

### Phase 3 : Features
- [ ] Pagination pour toutes les routes
- [ ] Rate limiting
- [ ] Auto-init schemas
- [ ] Migration tooling
- [ ] OpenAPI documentation

### Phase 4 : Ops
- [ ] Healthcheck endpoints
- [ ] Graceful shutdown
- [ ] Configuration externalisée (env vars)
- [ ] Monitoring dashboards
- [ ] Alerting

## 🎓 Conclusion

L'architecture `this-rs` offre une base solide pour construire des microservices scalables avec :

✅ Séparation claire core/client  
✅ Configuration déclarative  
✅ Auth granulaire  
✅ Multi-tenancy natif  
✅ Prêt pour ScyllaDB  
✅ Extensible (Neo4j, caching, etc.)

Le framework est **production-ready** avec les implémentations ScyllaDB et Auth complétées.

