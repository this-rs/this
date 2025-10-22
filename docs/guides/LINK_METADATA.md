# Métadonnées de Liens et Mises à Jour

## Vue d'Ensemble

Les **liens** dans `this-rs` ne sont pas de simples relations binaires. Ils peuvent **porter des données (metadata)** et être **mis à jour** au fil du temps, permettant de stocker un contexte riche sur les relations entre entités.

## Pourquoi les Liens Portent des Données ?

### Cas d'Usage Réels

#### 1. **Relation d'Emploi** (User → Company)
```json
{
  "link_type": "worker",
  "source": { "id": "user-123", "entity_type": "user" },
  "target": { "id": "company-456", "entity_type": "company" },
  "metadata": {
    "role": "Senior Developer",
    "department": "Engineering",
    "start_date": "2024-01-01",
    "salary": 85000,
    "employment_type": "full-time"
  }
}
```

#### 2. **Propriété avec Historique** (User → Car)
```json
{
  "link_type": "owner",
  "metadata": {
    "purchase_date": "2023-06-15",
    "purchase_price": 35000,
    "purchase_location": "Paris",
    "ownership_share": 100
  }
}
```

#### 3. **Paiement** (Invoice → Payment)
```json
{
  "link_type": "payment",
  "metadata": {
    "transaction_id": "txn_abc123",
    "payment_method": "credit_card",
    "paid_amount": 1250.00,
    "payment_date": "2024-10-20",
    "status": "completed",
    "card_last4": "4242"
  }
}
```

#### 4. **Approbation Workflow** (Document → User)
```json
{
  "link_type": "approval",
  "metadata": {
    "approved_at": "2024-10-22T10:30:00Z",
    "approval_level": "manager",
    "comments": "Approved with minor modifications",
    "approval_status": "approved"
  }
}
```

## Structure d'un Link

```rust
pub struct Link {
    pub id: Uuid,                          // ID unique du lien
    pub tenant_id: Uuid,                   // Isolation multi-tenant
    pub link_type: String,                 // Type de relation
    pub source: EntityReference,           // Entité source
    pub target: EntityReference,           // Entité target
    pub metadata: Option<serde_json::Value>, // 🔥 Données du lien !
    pub created_at: DateTime<Utc>,         // Date de création
    pub updated_at: DateTime<Utc>,         // Date de mise à jour
}
```

## Opérations CRUD sur les Liens

### 1. **Créer un Lien avec Metadata**

#### HTTP Request
```bash
POST /users/{user_id}/worker/companies/{company_id}
Content-Type: application/json

{
  "metadata": {
    "role": "Developer",
    "start_date": "2024-01-01",
    "department": "Engineering"
  }
}
```

#### Response (201 Created)
```json
{
  "id": "link-abc123",
  "tenant_id": "tenant-xyz",
  "link_type": "worker",
  "source": {
    "id": "user-123",
    "entity_type": "user"
  },
  "target": {
    "id": "company-456",
    "entity_type": "company"
  },
  "metadata": {
    "role": "Developer",
    "start_date": "2024-01-01",
    "department": "Engineering"
  },
  "created_at": "2024-10-22T10:00:00Z",
  "updated_at": "2024-10-22T10:00:00Z"
}
```

### 2. **Mettre à Jour un Lien** 🆕

#### HTTP Request
```bash
PUT /users/{user_id}/worker/companies/{company_id}
Content-Type: application/json

{
  "metadata": {
    "role": "Senior Developer",
    "start_date": "2024-01-01",
    "department": "Engineering",
    "promotion_date": "2024-06-01",
    "salary": 95000
  }
}
```

#### Response (200 OK)
```json
{
  "id": "link-abc123",
  "tenant_id": "tenant-xyz",
  "link_type": "worker",
  "source": { "id": "user-123", "entity_type": "user" },
  "target": { "id": "company-456", "entity_type": "company" },
  "metadata": {
    "role": "Senior Developer",
    "start_date": "2024-01-01",
    "department": "Engineering",
    "promotion_date": "2024-06-01",
    "salary": 95000
  },
  "created_at": "2024-10-22T10:00:00Z",
  "updated_at": "2024-10-22T12:30:00Z"  // ← Timestamp mis à jour
}
```

### 3. **Lister les Liens avec Metadata**

```bash
GET /users/{user_id}/companies-work
```

```json
{
  "links": [
    {
      "id": "link-abc123",
      "link_type": "worker",
      "target": { "id": "company-456", "entity_type": "company" },
      "metadata": {
        "role": "Senior Developer",
        "promotion_date": "2024-06-01"
      },
      "created_at": "2024-10-22T10:00:00Z",
      "updated_at": "2024-10-22T12:30:00Z"
    }
  ],
  "count": 1
}
```

### 4. **Supprimer un Lien**

```bash
DELETE /users/{user_id}/worker/companies/{company_id}
```

## Permissions pour Update

Les permissions de mise à jour peuvent être définies **par type de lien** :

```yaml
links:
  - link_type: worker
    source_type: user
    target_type: company
    auth:
      list: authenticated
      create: role:hr           # Seul HR peut créer l'emploi
      update: source_owner      # L'employé peut mettre à jour ses infos
      delete: role:hr
  
  - link_type: approval
    source_type: document
    target_type: user
    auth:
      list: authenticated
      create: role:manager      # Manager crée l'approbation
      update: role:manager      # Manager peut modifier
      delete: admin_only
```

## Exemples d'Utilisation

### Scenario 1 : Gestion d'Emploi

#### Étape 1 : HR embauche un employé
```bash
POST /users/john-123/worker/companies/acme-inc
{
  "metadata": {
    "role": "Junior Developer",
    "start_date": "2024-01-01",
    "contract_type": "CDI"
  }
}
```

#### Étape 2 : Promotion après 6 mois
```bash
PUT /users/john-123/worker/companies/acme-inc
{
  "metadata": {
    "role": "Developer",
    "start_date": "2024-01-01",
    "contract_type": "CDI",
    "promotion_date": "2024-06-01",
    "previous_role": "Junior Developer"
  }
}
```

#### Étape 3 : Promotion Senior
```bash
PUT /users/john-123/worker/companies/acme-inc
{
  "metadata": {
    "role": "Senior Developer",
    "start_date": "2024-01-01",
    "contract_type": "CDI",
    "promotion_date": "2025-01-01",
    "previous_role": "Developer",
    "team_lead": true
  }
}
```

### Scenario 2 : Workflow de Paiement

#### Étape 1 : Créer le paiement (pending)
```bash
POST /invoices/inv-123/payment/payments/pay-456
{
  "metadata": {
    "status": "pending",
    "payment_method": "credit_card",
    "created_by": "user-789"
  }
}
```

#### Étape 2 : Mettre à jour après traitement
```bash
PUT /invoices/inv-123/payment/payments/pay-456
{
  "metadata": {
    "status": "processing",
    "payment_method": "credit_card",
    "created_by": "user-789",
    "transaction_id": "txn_abc123",
    "processing_started_at": "2024-10-22T10:00:00Z"
  }
}
```

#### Étape 3 : Confirmer le paiement
```bash
PUT /invoices/inv-123/payment/payments/pay-456
{
  "metadata": {
    "status": "completed",
    "payment_method": "credit_card",
    "created_by": "user-789",
    "transaction_id": "txn_abc123",
    "processing_started_at": "2024-10-22T10:00:00Z",
    "completed_at": "2024-10-22T10:05:00Z",
    "confirmation_code": "CONF-XYZ"
  }
}
```

### Scenario 3 : Historique de Propriété

#### Achat initial
```bash
POST /users/alice-123/owner/cars/tesla-456
{
  "metadata": {
    "purchase_date": "2023-01-15",
    "purchase_price": 45000,
    "purchase_location": "Tesla Center Paris"
  }
}
```

#### Ajout d'informations après coup
```bash
PUT /users/alice-123/owner/cars/tesla-456
{
  "metadata": {
    "purchase_date": "2023-01-15",
    "purchase_price": 45000,
    "purchase_location": "Tesla Center Paris",
    "financing": {
      "type": "loan",
      "duration_months": 60,
      "monthly_payment": 750
    },
    "insurance": {
      "company": "AXA",
      "policy_number": "POL-123456"
    }
  }
}
```

## Implémentation dans le Code

### Trait LinkService

```rust
#[async_trait]
pub trait LinkService: Send + Sync {
    // Créer un lien avec metadata
    async fn create(
        &self,
        tenant_id: &Uuid,
        link_type: &str,
        source: EntityReference,
        target: EntityReference,
        metadata: Option<serde_json::Value>,  // ← Metadata initiale
    ) -> Result<Link>;

    // Mettre à jour la metadata d'un lien
    async fn update(
        &self,
        tenant_id: &Uuid,
        id: &Uuid,
        metadata: Option<serde_json::Value>,  // ← Nouvelle metadata
    ) -> Result<Link>;

    // Autres méthodes...
}
```

### Handler HTTP

```rust
pub async fn update_link(
    State(state): State<AppState>,
    Path((source_type, source_id, route_name, target_id)): Path<(String, Uuid, String, Uuid)>,
    headers: HeaderMap,
    Json(payload): Json<CreateLinkRequest>,
) -> Result<Response, ExtractorError> {
    // 1. Extraire tenant_id
    let tenant_id = extract_tenant_id(&headers)?;
    
    // 2. Résoudre le route_name vers link_definition
    let extractor = DirectLinkExtractor::from_path(
        (source_type, source_id, route_name, target_id),
        &state.registry,
        &state.config,
        tenant_id,
    )?;
    
    // 3. Vérifier les permissions (TODO)
    // check_auth_policy(&headers, &extractor.link_definition.auth.update, ...)?;
    
    // 4. Trouver le lien existant
    let existing_link = find_link(...).await?;
    
    // 4. Mettre à jour la metadata
    let updated_link = state.link_service
        .update(&tenant_id, &existing_link.id, payload.metadata)
        .await?;
    
    Ok(Json(updated_link).into_response())
}
```

## Types de Metadata Recommandés

### 1. **Metadata Temporelle**
```json
{
  "start_date": "2024-01-01",
  "end_date": null,
  "duration": "6 months",
  "renewal_date": "2024-07-01"
}
```

### 2. **Metadata de Status**
```json
{
  "status": "active",
  "status_history": [
    { "status": "pending", "date": "2024-01-01" },
    { "status": "active", "date": "2024-01-15" }
  ]
}
```

### 3. **Metadata Financière**
```json
{
  "amount": 1250.00,
  "currency": "EUR",
  "payment_method": "credit_card",
  "transaction_id": "txn_abc123"
}
```

### 4. **Metadata de Permission**
```json
{
  "role": "editor",
  "permissions": ["read", "write", "delete"],
  "granted_by": "admin-123",
  "granted_at": "2024-01-01"
}
```

### 5. **Metadata Hiérarchique**
```json
{
  "position": "Engineering Manager",
  "reports_to": "cto-456",
  "manages": ["team-frontend", "team-backend"],
  "level": "senior"
}
```

## Bonnes Pratiques

### ✅ DO

1. **Inclure des timestamps** dans la metadata pour tracer les changements
2. **Utiliser des structures cohérentes** pour faciliter les requêtes
3. **Documenter les champs** dans le fichier de configuration
4. **Valider la metadata** côté application (pas encore dans le framework)
5. **Conserver l'historique** si nécessaire (créer de nouveaux liens au lieu de mettre à jour)

### ❌ DON'T

1. **Ne pas stocker des données sensibles** non chiffrées (mots de passe, etc.)
2. **Ne pas dépasser ~1KB** de metadata (performances)
3. **Ne pas dupliquer** des données qui devraient être dans les entités
4. **Ne pas utiliser metadata** pour des relations complexes (créer des entités)

## Avantages

### 1. **Flexibilité**
Chaque relation peut avoir son propre contexte sans créer de nouvelles tables.

### 2. **Historisation Facile**
Le champ `updated_at` permet de tracer les modifications.

### 3. **Requêtes Simplifiées**
Tout le contexte de la relation est dans un seul objet JSON.

### 4. **Évolutivité**
Ajouter des champs dans la metadata ne nécessite pas de migration de schéma.

### 5. **Type-Safety Optionnelle**
L'application peut définir des types TypeScript/Rust pour valider la metadata.

## Routes Disponibles

| Méthode | Route | Description |
|---------|-------|-------------|
| `POST` | `/{source}/{source_id}/{link_type}/{target}/{target_id}` | Créer un lien avec metadata |
| `PUT` | `/{source}/{source_id}/{link_type}/{target}/{target_id}` | Mettre à jour la metadata 🆕 |
| `GET` | `/{source}/{source_id}/{route_name}` | Lister les liens (avec metadata) |
| `DELETE` | `/{source}/{source_id}/{link_type}/{target}/{target_id}` | Supprimer un lien |

## Tests

Le framework inclut des tests pour les mises à jour :

```rust
#[tokio::test]
async fn test_update_link() {
    let service = InMemoryLinkService::new();
    let tenant_id = Uuid::new_v4();
    
    // Create with initial metadata
    let link = service.create(
        &tenant_id,
        "worker",
        source,
        target,
        Some(serde_json::json!({"role": "Developer"})),
    ).await.unwrap();
    
    // Update metadata
    let updated = service.update(
        &tenant_id,
        &link.id,
        Some(serde_json::json!({
            "role": "Senior Developer",
            "promotion_date": "2024-06-01"
        })),
    ).await.unwrap();
    
    assert_eq!(updated.metadata, Some(...));
    assert_ne!(updated.updated_at, link.updated_at);
}
```

## Conclusion

Les **liens avec metadata** permettent de modéliser des relations riches et évolutives, essentielles pour des applications professionnelles.

**Points clés** :
- ✅ Les liens peuvent porter des données JSON arbitraires
- ✅ La metadata peut être mise à jour via `PUT`
- ✅ Les permissions `update` sont configurables par lien
- ✅ Le champ `updated_at` est automatiquement mis à jour
- ✅ Idéal pour workflow, historique, permissions, etc.

**Voir aussi** :
- [LINK_AUTHORIZATION.md](LINK_AUTHORIZATION.md) - Permissions des liens
- [examples/microservice/config/links.yaml](../../examples/microservice/config/links.yaml) - Configuration avec update

