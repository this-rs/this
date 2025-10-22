# Liens Enrichis avec Entités Complètes

## Vue d'Ensemble

Par défaut, les liens dans `this-rs` retournent maintenant **automatiquement les entités complètes** au lieu de simples références, évitant ainsi le problème des requêtes N+1.

De plus, le système est **intelligent** : il n'inclut que les entités dont vous avez besoin selon le contexte de votre requête.

## Les Trois Contextes d'Enrichissement

### 1. Navigation depuis la Source (Forward)

**Route** : `GET /{source_type}/{source_id}/{route_name}`

**Exemple** : `GET /orders/123/invoices`

Puisque vous connaissez déjà l'order (il est dans l'URL), seules les **entités target** (invoices) sont retournées.

#### Requête
```bash
curl -H 'X-Tenant-ID: abc-123' \
  http://localhost:3000/orders/962b87e4-65cf-4802-bc3c-d1866923f137/invoices
```

#### Réponse
```json
{
  "links": [
    {
      "id": "link-789",
      "tenant_id": "abc-123",
      "link_type": "has_invoice",
      "target": {
        "id": "7c885c25-6797-43b5-af64-f256d284d152",
        "number": "INV-001",
        "amount": 1500.0,
        "status": "sent",
        "due_date": "2025-11-15",
        "paid_at": null
      },
      "metadata": {
        "created_at": "2025-10-20T10:00:00Z",
        "created_by": "system",
        "invoice_type": "standard"
      },
      "created_at": "2025-10-22T13:18:22.289068Z",
      "updated_at": "2025-10-22T13:18:22.289068Z"
    }
  ],
  "count": 1,
  "link_type": "has_invoice",
  "direction": "Forward"
}
```

**Note** : Pas de champ `source` car vous connaissez déjà l'order !

---

### 2. Navigation depuis la Target (Reverse)

**Route** : `GET /{target_type}/{target_id}/{route_name}`

**Exemple** : `GET /payments/456/invoice`

Puisque vous connaissez déjà le payment (il est dans l'URL), seule l'**entité source** (invoice) est retournée.

#### Requête
```bash
curl -H 'X-Tenant-ID: abc-123' \
  http://localhost:3000/payments/ee24aec0-c27d-41e0-a61f-3e0a5d5a62ab/invoice
```

#### Réponse
```json
{
  "links": [
    {
      "id": "link-456",
      "tenant_id": "abc-123",
      "link_type": "payment",
      "source": {
        "id": "b0f8c69e-8d2c-4969-87da-0682a7440bd5",
        "number": "INV-002",
        "amount": 1500.0,
        "status": "paid",
        "due_date": "2025-11-20",
        "paid_at": "2025-10-20"
      },
      "metadata": {
        "payment_date": "2025-10-20T15:45:00Z",
        "payment_method": "card",
        "transaction_id": "txn_1234567890"
      },
      "created_at": "2025-10-22T13:18:22.289347Z",
      "updated_at": "2025-10-22T13:18:22.289347Z"
    }
  ],
  "count": 1,
  "link_type": "payment",
  "direction": "Reverse"
}
```

**Note** : Pas de champ `target` car vous connaissez déjà le payment !

---

### 3. Accès Direct au Lien

**Route** : `GET /links/{link_id}`

**Exemple** : `GET /links/12b283f7-de7d-41d0-9249-3c108d6453fa`

Quand vous accédez directement à un lien par son ID, vous ne savez pas quelles entités sont concernées. Donc **les deux entités complètes** (source ET target) sont retournées.

#### Requête
```bash
curl -H 'X-Tenant-ID: abc-123' \
  http://localhost:3000/links/12b283f7-de7d-41d0-9249-3c108d6453fa
```

#### Réponse
```json
{
  "id": "12b283f7-de7d-41d0-9249-3c108d6453fa",
  "tenant_id": "abc-123",
  "link_type": "payment",
  "source": {
    "id": "b0f8c69e-8d2c-4969-87da-0682a7440bd5",
    "number": "INV-002",
    "amount": 1500.0,
    "status": "paid",
    "due_date": "2025-11-20",
    "paid_at": "2025-10-20"
  },
  "target": {
    "id": "ee24aec0-c27d-41e0-a61f-3e0a5d5a62ab",
    "number": "PAY-001",
    "amount": 1500.0,
    "method": "card",
    "status": "completed",
    "transaction_id": "txn_1234567890"
  },
  "metadata": {
    "payment_date": "2025-10-20T15:45:00Z",
    "payment_method": "card",
    "transaction_id": "txn_1234567890"
  },
  "created_at": "2025-10-22T13:18:22.289347Z",
  "updated_at": "2025-10-22T13:18:22.289347Z"
}
```

**Note** : Les deux champs `source` et `target` sont présents !

---

## Avantages

### ✅ Performance

**Avant** (sans enrichissement) :
```javascript
// Récupérer les invoices d'un order
const response = await fetch('/orders/123/invoices');
const { links } = await response.json();

// Pour chaque invoice, faire une requête supplémentaire
for (const link of links) {
  const invoice = await fetch(`/invoices/${link.target.id}`);
  // ... utiliser invoice
}
// Total: 1 + N requêtes
```

**Après** (avec enrichissement) :
```javascript
// Une seule requête suffit !
const response = await fetch('/orders/123/invoices');
const { links } = await response.json();

// Les invoices complètes sont déjà là
for (const link of links) {
  console.log(link.target); // Entité complète !
}
// Total: 1 requête
```

### ✅ Pas de Redondance

Le système n'envoie **jamais** de données redondantes :
- Si vous naviguez depuis `/orders/{id}/invoices`, vous connaissez déjà l'order
- Si vous naviguez depuis `/payments/{id}/invoice`, vous connaissez déjà le payment

### ✅ Bande Passante Optimisée

Pour 10 liens :

| Contexte | Entités chargées | Économie |
|----------|------------------|----------|
| Forward Navigation | 0 sources + 10 targets = **10 entités** | 50% |
| Reverse Navigation | 10 sources + 0 targets = **10 entités** | 50% |
| Direct Link | 1 source + 1 target = **2 entités** | N/A |

**Avant** : 20 entités chargées
**Après** : 10 entités chargées (en moyenne)

---

## Structure `EnrichedLink`

```rust
pub struct EnrichedLink {
    pub id: Uuid,
    pub tenant_id: Uuid,
    pub link_type: String,
    
    /// Entité source complète (omise si navigation depuis source)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<serde_json::Value>,
    
    /// Entité target complète (omise si navigation depuis target)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<serde_json::Value>,
    
    pub metadata: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
```

Les champs `source` et `target` sont **optionnels** et automatiquement omis du JSON si `None`.

---

## Comment ça Marche ?

### Architecture

Le système utilise le **trait `EntityFetcher`** pour charger dynamiquement n'importe quelle entité :

```rust
#[async_trait]
pub trait EntityFetcher: Send + Sync {
    async fn fetch_as_json(
        &self,
        tenant_id: &Uuid,
        entity_id: &Uuid,
    ) -> Result<serde_json::Value>;
}
```

### Enregistrement des Fetchers

Chaque module expose ses fetchers via la méthode `get_entity_fetcher()` :

```rust
impl Module for BillingModule {
    fn get_entity_fetcher(&self, entity_type: &str) -> Option<Arc<dyn EntityFetcher>> {
        match entity_type {
            "order" => Some(Arc::new(self.store.orders.clone())),
            "invoice" => Some(Arc::new(self.store.invoices.clone())),
            "payment" => Some(Arc::new(self.store.payments.clone())),
            _ => None,
        }
    }
}
```

### Implémentation pour un Store

```rust
#[async_trait]
impl EntityFetcher for OrderStore {
    async fn fetch_as_json(
        &self,
        tenant_id: &Uuid,
        entity_id: &Uuid,
    ) -> Result<serde_json::Value> {
        let order = self.get(entity_id)
            .ok_or_else(|| anyhow!("Order not found"))?;
        
        // Vérifier l'isolation tenant
        if order.tenant_id != *tenant_id {
            anyhow::bail!("Access denied");
        }
        
        // Sérialiser en JSON
        Ok(serde_json::to_value(order)?)
    }
}
```

### Enrichissement Contextuel

Le handler `list_links` détermine automatiquement le contexte :

```rust
let context = match extractor.direction {
    LinkDirection::Forward => EnrichmentContext::FromSource,
    LinkDirection::Reverse => EnrichmentContext::FromTarget,
};

let enriched_links = enrich_links_with_entities(
    &state,
    links,
    &tenant_id,
    context  // 🔥 Contexte intelligent !
).await?;
```

La fonction `enrich_links_with_entities()` charge uniquement les entités nécessaires :

```rust
for link in links {
    let source_entity = match context {
        EnrichmentContext::FromSource => None,  // Déjà connu
        _ => Some(fetch_entity_by_type(...).await?)
    };
    
    let target_entity = match context {
        EnrichmentContext::FromTarget => None,  // Déjà connu
        _ => Some(fetch_entity_by_type(...).await?)
    };
    
    enriched.push(EnrichedLink {
        source: source_entity,
        target: target_entity,
        ...
    });
}
```

---

## Ajouter le Support pour une Nouvelle Entité

### 1. Implémenter `EntityFetcher`

```rust
// Dans votre store
#[async_trait]
impl EntityFetcher for ProductStore {
    async fn fetch_as_json(
        &self,
        tenant_id: &Uuid,
        entity_id: &Uuid,
    ) -> Result<serde_json::Value> {
        let product = self.get(entity_id)
            .ok_or_else(|| anyhow!("Product not found"))?;
        
        if product.tenant_id != *tenant_id {
            anyhow::bail!("Access denied");
        }
        
        Ok(serde_json::to_value(product)?)
    }
}
```

### 2. Enregistrer dans le Module

```rust
impl Module for YourModule {
    fn get_entity_fetcher(&self, entity_type: &str) -> Option<Arc<dyn EntityFetcher>> {
        match entity_type {
            "product" => Some(Arc::new(self.store.products.clone())),
            _ => None,
        }
    }
}
```

C'est tout ! Le système gère automatiquement l'enrichissement.

---

## Cas d'Usage Avancés

### Filtrer les Champs Retournés

Si vous voulez retourner une vue partielle d'une entité :

```rust
#[async_trait]
impl EntityFetcher for UserStore {
    async fn fetch_as_json(&self, tenant_id: &Uuid, entity_id: &Uuid) 
        -> Result<serde_json::Value> 
    {
        let user = self.get(entity_id)?;
        
        // Retourner seulement les champs publics
        Ok(json!({
            "id": user.id,
            "name": user.name,
            "email": user.email,
            // ❌ Pas de password, pas de internal_notes, etc.
        }))
    }
}
```

### Inclure des Champs Calculés

```rust
#[async_trait]
impl EntityFetcher for OrderStore {
    async fn fetch_as_json(&self, tenant_id: &Uuid, entity_id: &Uuid) 
        -> Result<serde_json::Value> 
    {
        let order = self.get(entity_id)?;
        
        // Ajouter des champs calculés
        let mut json = serde_json::to_value(&order)?;
        json["total_with_tax"] = json!(order.amount * 1.20);
        json["is_overdue"] = json!(order.due_date < Utc::now());
        
        Ok(json)
    }
}
```

---

## Comparaison avec GraphQL

L'enrichissement automatique de `this-rs` offre des avantages similaires à GraphQL, mais **sans la complexité** :

| Feature | this-rs | GraphQL |
|---------|---------|---------|
| Évite N+1 | ✅ Automatique | ✅ Via DataLoader |
| Pas de redondance | ✅ Contextuel | ⚠️ Dépend du query |
| Configuration | ✅ Zero config | ❌ Schema complexe |
| Type-safety | ✅ Rust | ⚠️ Codegen requis |
| Courbe d'apprentissage | ✅ Simple | ❌ Élevée |

---

## Conclusion

L'enrichissement automatique des liens est une fonctionnalité **puissante et transparente** qui :

- ✅ Élimine les requêtes N+1
- ✅ Optimise la bande passante
- ✅ Simplifie le code client
- ✅ Préserve la généricité du framework
- ✅ S'adapte intelligemment au contexte

Tout cela **sans configuration** et **sans boilerplate** ! 🚀

