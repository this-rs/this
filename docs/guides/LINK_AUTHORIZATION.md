# Link-Level Authorization

## Vue d'Ensemble

Le framework `this-rs` supporte maintenant l'**autorisation au niveau des liens** (link-level authorization), permettant de définir des permissions spécifiques pour chaque type de lien, indépendamment des permissions des entités source et target.

## Pourquoi l'Autorisation au Niveau des Liens ?

### Problématique

Auparavant, les permissions de liens étaient définies **uniquement au niveau des entités** :

```yaml
entities:
  - singular: order
    plural: orders
    auth:
      create_link: owner    # ← Même permission pour TOUS les types de liens
      delete_link: owner
```

**Problème** : Un `order` peut avoir différents types de liens avec différentes exigences de sécurité :
- `order → invoice` : Créé automatiquement par le système (service_only)
- `order → user_approval` : Créé manuellement par le propriétaire (owner)

Avec l'ancienne approche, les deux héritaient de la même permission, ce qui n'était pas idéal.

### Solution : Auth par Link

Maintenant, chaque **LinkDefinition** peut avoir sa propre configuration d'autorisation :

```yaml
links:
  - link_type: has_invoice
    source_type: order
    target_type: invoice
    auth:
      list: authenticated      # Liste accessible aux utilisateurs authentifiés
      create: service_only     # Création réservée aux services internes
      delete: admin_only       # Suppression réservée aux admins
  
  - link_type: approval
    source_type: order
    target_type: user
    auth:
      list: owner              # Seul le propriétaire peut lister
      create: owner            # Seul le propriétaire peut créer
      delete: owner            # Seul le propriétaire peut supprimer
```

## Configuration YAML

### Structure Complète

```yaml
entities:
  - singular: order
    plural: orders
    auth:
      list: authenticated
      get: authenticated
      create: authenticated
      update: owner
      delete: owner
      # Permissions fallback pour les liens sans auth spécifique
      list_links: authenticated
      create_link: owner
      delete_link: owner

links:
  # Lien avec auth spécifique
  - link_type: has_invoice
    source_type: order
    target_type: invoice
    forward_route_name: invoices
    reverse_route_name: order
    description: "Order has invoices"
    auth:                              # ← Auth spécifique au lien
      list: authenticated              # Qui peut lister les invoices d'un order
      create: service_only             # Qui peut créer ce lien
      delete: admin_only               # Qui peut supprimer ce lien
  
  # Lien sans auth spécifique (utilise les permissions de l'entité)
  - link_type: other_link
    source_type: order
    target_type: something
    forward_route_name: somethings
    reverse_route_name: order
    # Pas de auth → utilise order.auth.create_link, etc.
```

### Politiques d'Autorisation Disponibles

| Politique | Description | Exemple d'Usage |
|-----------|-------------|-----------------|
| `public` | Accessible sans authentification | Données publiques |
| `authenticated` | Requiert authentification | Accès standard |
| `owner` | Seul le propriétaire | Données personnelles |
| `service_only` | Services internes uniquement | Liens automatiques |
| `admin_only` | Administrateurs uniquement | Opérations sensibles |
| `role:manager` | Rôle spécifique requis | Permissions métier |
| `owner_or_service` | Propriétaire OU service | Flexibilité |
| `source_owner` | Propriétaire de la source | Liens sortants |
| `target_owner` | Propriétaire de la target | Liens entrants |
| `source_owner_or_target_owner` | L'un ou l'autre | Liens bidirectionnels |

## Comportement de Fallback

Si un lien **n'a pas** de configuration `auth`, le système utilise les permissions de l'entité source :

```yaml
entities:
  - singular: order
    auth:
      create_link: owner    # ← Fallback utilisé

links:
  - link_type: some_link
    source_type: order
    target_type: target
    # Pas de auth → utilise order.auth.create_link = owner
```

## Exemples d'Utilisation

### Cas 1 : Liens Automatiques vs Manuels

```yaml
links:
  # Facture créée automatiquement par le système
  - link_type: has_invoice
    source_type: order
    target_type: invoice
    auth:
      create: service_only     # ← Seuls les services peuvent créer
      delete: admin_only       # ← Seuls les admins peuvent supprimer
  
  # Note ajoutée manuellement par l'utilisateur
  - link_type: has_note
    source_type: order
    target_type: note
    auth:
      create: owner            # ← Le propriétaire peut créer
      delete: owner            # ← Le propriétaire peut supprimer
```

### Cas 2 : Visibilité Différente

```yaml
links:
  # Paiements visibles seulement par le propriétaire
  - link_type: payment
    source_type: invoice
    target_type: payment
    auth:
      list: owner              # ← Seul le propriétaire voit les paiements
      create: owner_or_service
      delete: admin_only
  
  # Historique visible par tous les utilisateurs authentifiés
  - link_type: audit_log
    source_type: invoice
    target_type: log_entry
    auth:
      list: authenticated      # ← Tous peuvent voir l'historique
      create: service_only
      delete: admin_only
```

### Cas 3 : Workflow Complexe

```yaml
links:
  # Manager crée l'approbation
  - link_type: approval_request
    source_type: order
    target_type: approval
    auth:
      list: authenticated
      create: role:manager     # ← Seul un manager peut demander
      delete: role:manager
  
  # Admin valide
  - link_type: approval_validated
    source_type: approval
    target_type: order
    auth:
      list: authenticated
      create: admin_only       # ← Seul un admin valide
      delete: admin_only
```

## Implémentation dans le Code

### Structure `LinkAuthConfig`

```rust
use crate::core::LinkAuthConfig;

// La config est automatiquement parsée depuis le YAML
pub struct LinkAuthConfig {
    pub list: String,     // Politique pour GET /{source}/{id}/{route}
    pub create: String,   // Politique pour POST /{source}/{id}/{link}/{target}/{id}
    pub delete: String,   // Politique pour DELETE /{source}/{id}/{link}/{target}/{id}
}
```

### Dans les Handlers

Les handlers vérifient automatiquement les permissions du lien :

```rust
// TODO: Cette vérification sera implémentée dans les handlers
pub async fn create_link(...) {
    // 1. Extraire la LinkDefinition
    let link_def = extractor.link_definition;
    
    // 2. Vérifier l'auth spécifique au lien
    if let Some(link_auth) = &link_def.auth {
        check_auth_policy(&headers, &link_auth.create, &extractor)?;
    } else {
        // 3. Fallback sur l'auth de l'entité
        check_entity_link_auth(&headers, &source_type, "create_link")?;
    }
    
    // 4. Créer le lien
    // ...
}
```

### Méthode Helper

```rust
use crate::links::handlers::AppState;

// Obtenir la politique d'auth pour un lien
let policy = AppState::get_link_auth_policy(&link_definition, "create");

match policy {
    Some(p) => println!("Using link-specific policy: {}", p),
    None => println!("Using entity fallback policy"),
}
```

## Tests Automatiques

Le framework inclut des tests pour valider le parsing :

```rust
#[test]
fn test_link_auth_config_parsing() {
    let yaml = r#"
links:
  - link_type: has_invoice
    source_type: order
    target_type: invoice
    forward_route_name: invoices
    reverse_route_name: order
    auth:
      list: authenticated
      create: service_only
      delete: admin_only
"#;

    let config = LinksConfig::from_yaml_str(yaml).unwrap();
    let link_def = &config.links[0];
    
    assert!(link_def.auth.is_some());
    let auth = link_def.auth.as_ref().unwrap();
    assert_eq!(auth.create, "service_only");
}
```

## Migration depuis l'Ancienne Version

### Avant (auth au niveau entity uniquement)

```yaml
entities:
  - singular: order
    auth:
      create_link: owner
      delete_link: owner

links:
  - link_type: has_invoice
    source_type: order
    target_type: invoice
    # Hérite de order.auth.create_link = owner
```

### Après (auth au niveau link)

```yaml
entities:
  - singular: order
    auth:
      create_link: owner    # Fallback par défaut
      delete_link: owner

links:
  - link_type: has_invoice
    source_type: order
    target_type: invoice
    auth:
      create: service_only  # ← Override pour ce lien spécifique
      delete: admin_only
  
  - link_type: other_link
    source_type: order
    target_type: other
    # Pas de auth → utilise le fallback order.auth.create_link = owner
```

**Migration graduelle** : Vous pouvez ajouter l'auth aux liens progressivement, les liens sans `auth` continuent de fonctionner avec les permissions d'entité.

## Avantages

### 1. **Granularité Fine**
Chaque type de lien peut avoir ses propres règles de sécurité.

### 2. **Séparation des Responsabilités**
- Liens système → `service_only`
- Liens utilisateur → `owner`
- Liens admin → `admin_only`

### 3. **Flexibilité**
Plusieurs types de liens entre les mêmes entités avec des permissions différentes.

### 4. **Sécurité Renforcée**
Empêche les utilisateurs de créer des liens qu'ils ne devraient pas pouvoir créer.

### 5. **Backward Compatible**
Les liens sans `auth` utilisent le comportement par défaut (fallback sur entity auth).

## Cas d'Usage Avancés

### Multi-Tenant avec Rôles

```yaml
links:
  # Seuls les services du même tenant
  - link_type: internal_process
    source_type: order
    target_type: workflow
    auth:
      list: service_only
      create: service_only
      delete: service_only
  
  # Propriétaire du tenant
  - link_type: owner_action
    source_type: order
    target_type: action
    auth:
      list: owner
      create: owner
      delete: owner
  
  # Admin de la plateforme
  - link_type: platform_admin
    source_type: order
    target_type: admin_log
    auth:
      list: admin_only
      create: admin_only
      delete: admin_only
```

### Workflow d'Approbation

```yaml
links:
  # Étape 1 : Demande (par employé)
  - link_type: approval_request
    source_type: expense
    target_type: approval
    auth:
      create: authenticated
      list: authenticated
      delete: source_owner
  
  # Étape 2 : Validation (par manager)
  - link_type: manager_approved
    source_type: approval
    target_type: expense
    auth:
      create: role:manager
      list: authenticated
      delete: role:manager
  
  # Étape 3 : Paiement (par finance)
  - link_type: payment_processed
    source_type: expense
    target_type: payment
    auth:
      create: role:finance
      list: owner
      delete: admin_only
```

## TODO : Implémentation Complète de l'Auth

⚠️ **Note** : Actuellement, le système de permissions est **défini mais pas encore appliqué** dans les handlers.

Les prochaines étapes d'implémentation incluront :
1. Middleware d'authentification Axum
2. Extraction du contexte utilisateur (user_id, roles, tenant_id)
3. Vérification des politiques dans les handlers
4. Tests d'intégration avec différents scénarios d'auth

Des commentaires `TODO` dans le code indiquent où l'auth doit être intégrée :

```rust
// TODO: Check authorization for link creation
// if let Some(link_def) = &extractor.link_definition {
//     if let Some(link_auth) = &link_def.auth {
//         check_auth_policy(&headers, &link_auth.create, &extractor)?;
//     }
// }
```

## Conclusion

L'autorisation au niveau des liens offre une flexibilité et une sécurité accrues pour gérer des workflows complexes, des rôles métier variés, et des scénarios multi-tenants avancés.

**Points clés** :
- ✅ Permissions granulaires par type de lien
- ✅ Fallback automatique sur entity auth
- ✅ Backward compatible
- ✅ Configuration déclarative en YAML
- ✅ Support de politiques complexes (roles, owner, service)
- ⏳ Implémentation complète de la vérification à venir

Pour plus d'informations, consultez :
- [GETTING_STARTED.md](GETTING_STARTED.md) - Guide de démarrage
- [QUICK_START.md](QUICK_START.md) - Référence rapide
- [examples/microservice/config/links.yaml](../../examples/microservice/config/links.yaml) - Exemple complet

