# Navigation Multi-Niveaux

## Question Initiale

> Il me semble que les permissions de création de lien devraient être au niveau des links et pas des entités.
> Mais que faudrait-il faire pour avoir la capacité de créer plusieurs niveaux d'imbrication :
> - company > section > employee

## Réponse

### Partie 1 : Permissions au Niveau des Links ✅

**Statut** : ✅ **IMPLÉMENTÉ**

Les permissions sont maintenant définies **directement dans les liens** via le champ `auth` dans `LinkDefinition`.

```yaml
links:
  - link_type: has_invoice
    source_type: order
    target_type: invoice
    auth:                        # ← Permissions au niveau du lien
      list: authenticated
      create: service_only
      delete: admin_only
```

**Voir** : [LINK_AUTHORIZATION.md](LINK_AUTHORIZATION.md) pour la documentation complète.

---

### Partie 2 : Navigation Multi-Niveaux ⏳

**Statut** : ⏳ **À IMPLÉMENTER**

Pour supporter la navigation `company > section > employee`, deux approches sont possibles :

## Approche 1 : Path Traversal (Recommandée) 🌟

### Concept

Permettre de traverser plusieurs niveaux de liens en une seule requête HTTP.

### Routes Désirées

```
GET /companies/{id}/sections                           # 1 niveau
GET /companies/{id}/sections/{section_id}/employees    # 2 niveaux
GET /sections/{id}/employees                           # 1 niveau
```

### Configuration YAML

```yaml
entities:
  - singular: company
    plural: companies
  - singular: section
    plural: sections
  - singular: employee
    plural: employees

links:
  # Company has sections
  - link_type: has_section
    source_type: company
    target_type: section
    forward_route_name: sections
    reverse_route_name: company
    auth:
      list: authenticated
      create: role:admin
      delete: role:admin
  
  # Section has employees
  - link_type: has_employee
    source_type: section
    target_type: employee
    forward_route_name: employees
    reverse_route_name: section
    auth:
      list: authenticated
      create: role:manager
      delete: role:hr
```

### Implémentation

#### 1. Handler de Traversal

```rust
// src/links/traversal.rs

/// Traverse multiple levels of links
///
/// Path format: /{entity1}/{id1}/{entity2}/{id2}/{entity3}
///
/// Example: /companies/123/sections/456/employees
pub async fn traverse_links(
    State(state): State<AppState>,
    Path(segments): Path<Vec<String>>,
    headers: HeaderMap,
) -> Result<Json<Vec<serde_json::Value>>, ExtractorError> {
    let tenant_id = extract_tenant_id(&headers)?;
    
    // Parse segments: ["companies", "123", "sections", "456", "employees"]
    if segments.len() < 3 || segments.len() % 2 == 0 {
        return Err(ExtractorError::InvalidPath(
            "Path must be: /{type1}/{id1}/{type2} or /{type1}/{id1}/{type2}/{id2}/{type3}".into()
        ));
    }
    
    let mut current_entity_type = &segments[0];
    let mut current_entity_id = parse_uuid(&segments[1])?;
    
    // Start with the first entity
    let mut path_index = 2;
    
    // Traverse to the final level
    while path_index < segments.len() - 1 {
        let next_type = &segments[path_index];
        let next_id = parse_uuid(&segments[path_index + 1])?;
        
        // Find the link definition between current and next
        let link_def = find_link_between(
            current_entity_type,
            next_type,
            &state.config,
        )?;
        
        // Verify the link exists
        verify_link_exists(
            &state.link_service,
            tenant_id,
            current_entity_id,
            current_entity_type,
            next_id,
            next_type,
            &link_def.link_type,
        ).await?;
        
        // Move to next level
        current_entity_type = next_type;
        current_entity_id = next_id;
        path_index += 2;
    }
    
    // Final segment is the target type to list
    let target_type = segments.last().unwrap();
    let link_def = find_link_between(
        current_entity_type,
        target_type,
        &state.config,
    )?;
    
    // Check authorization for listing
    if let Some(auth) = &link_def.auth {
        check_auth_policy(&headers, &auth.list, &state)?;
    }
    
    // Get all links from current entity to target
    let links = state.link_service
        .find_by_source(
            &tenant_id,
            &current_entity_id,
            current_entity_type,
            Some(&link_def.link_type),
            Some(target_type),
        )
        .await?;
    
    // Convert to JSON response
    let results = links.into_iter()
        .map(|link| {
            serde_json::json!({
                "id": link.target.id,
                "type": link.target.entity_type,
                "link_type": link.link_type,
            })
        })
        .collect();
    
    Ok(Json(results))
}

/// Find a link definition between two entity types
fn find_link_between(
    source_type: &str,
    target_type: &str,
    config: &LinksConfig,
) -> Result<LinkDefinition, ExtractorError> {
    // Convert plurals to singulars
    let source_singular = config
        .entities
        .iter()
        .find(|e| e.plural == source_type)
        .map(|e| e.singular.as_str())
        .unwrap_or(source_type);
    
    let target_singular = config
        .entities
        .iter()
        .find(|e| e.plural == target_type)
        .map(|e| e.singular.as_str())
        .unwrap_or(target_type);
    
    // Find link definition
    config
        .links
        .iter()
        .find(|def| {
            def.source_type == source_singular && def.target_type == target_singular
        })
        .cloned()
        .ok_or_else(|| {
            ExtractorError::RouteNotFound(format!(
                "No link definition found between {} and {}",
                source_type, target_type
            ))
        })
}

/// Verify a link exists between two specific entities
async fn verify_link_exists(
    link_service: &Arc<dyn LinkService>,
    tenant_id: Uuid,
    source_id: Uuid,
    source_type: &str,
    target_id: Uuid,
    target_type: &str,
    link_type: &str,
) -> Result<(), ExtractorError> {
    let links = link_service
        .find_by_source(&tenant_id, &source_id, source_type, Some(link_type), Some(target_type))
        .await
        .map_err(|e| ExtractorError::JsonError(e.to_string()))?;
    
    // Check if the specific target exists in the links
    let exists = links.iter().any(|link| link.target.id == target_id);
    
    if !exists {
        return Err(ExtractorError::RouteNotFound(format!(
            "Link not found: {} {} -> {} {}",
            source_type, source_id, target_type, target_id
        )));
    }
    
    Ok(())
}
```

#### 2. Enregistrer la Route

```rust
// src/server/router.rs

pub fn build_link_routes(state: AppState) -> Router {
    Router::new()
        .route("/:entity_type/:entity_id/:route_name", get(list_links))
        
        // 🆕 Routes multi-niveaux
        .route(
            "/:entity1/:id1/:entity2/:id2/:entity3",
            get(traversal::traverse_links)
        )
        .route(
            "/:entity1/:id1/:entity2/:id2/:entity3/:id3/:entity4",
            get(traversal::traverse_links)
        )
        
        .route(
            "/:source_type/:source_id/:link_type/:target_type/:target_id",
            post(create_link).delete(delete_link)
        )
        .route("/:entity_type/:entity_id/links", get(list_available_links))
        .with_state(state)
}
```

### Utilisation

#### Exemples de Requêtes

```bash
# 1 niveau : Lister les sections d'une company
GET /companies/123e4567-e89b-12d3-a456-426614174000/sections

# 2 niveaux : Lister les employees d'une section spécifique d'une company
GET /companies/123e4567-e89b-12d3-a456-426614174000/sections/789e4567-e89b-12d3-a456-426614174000/employees

# 1 niveau : Lister les employees d'une section (sans passer par company)
GET /sections/789e4567-e89b-12d3-a456-426614174000/employees
```

#### Réponse

```json
{
  "results": [
    {
      "id": "abc12345-e89b-12d3-a456-426614174000",
      "type": "employee",
      "link_type": "has_employee"
    },
    {
      "id": "def67890-e89b-12d3-a456-426614174000",
      "type": "employee",
      "link_type": "has_employee"
    }
  ],
  "path": "/companies/123.../sections/789.../employees",
  "depth": 2
}
```

### Avantages

✅ **Flexibilité** : Supporte n'importe quelle profondeur  
✅ **RESTful** : URLs claires et intuitives  
✅ **Sécurité** : Vérifie que chaque lien existe  
✅ **Authorization** : Respecte les permissions de chaque lien  
✅ **Performance** : Peut être optimisé avec des JOINs  

### Inconvénients

⚠️ **Complexité** : Parsing d'URL dynamique  
⚠️ **N+1 queries** : Peut nécessiter plusieurs appels DB  
⚠️ **Limite de profondeur** : À définir (max 5 niveaux ?)  

---

## Approche 2 : Nested Resources (Plus Complexe)

### Concept

Définir explicitement les routes imbriquées dans la configuration.

### Configuration YAML

```yaml
nested_routes:
  - name: company_section_employees
    path: /companies/{company_id}/sections/{section_id}/employees
    chain:
      - from: company
        to: section
        link_type: has_section
      - from: section
        to: employee
        link_type: has_employee
    auth: authenticated
```

### Avantages

✅ **Explicite** : Routes documentées dans la config  
✅ **Optimisable** : Peut générer des requêtes SQL optimisées  
✅ **Contrôle fin** : Auth globale sur la route complète  

### Inconvénients

⚠️ **Verbosité** : Chaque route imbriquée doit être définie  
⚠️ **Rigidité** : Pas de traversal dynamique  
⚠️ **Maintenance** : Plus de configuration à maintenir  

---

## Recommandation Finale

### Phase 1 : Implémenter Path Traversal ✅

**Priorité** : Haute  
**Effort** : Moyen (3-5 jours)  
**Bénéfice** : Résout 90% des cas d'usage

### Phase 2 : Optimisations (Optionnel)

**Priorité** : Basse  
**Effort** : Élevé  
**Bénéfice** : Performance pour cas complexes

Options d'optimisation :
1. **Batching** : Récupérer plusieurs niveaux en une seule query
2. **Caching** : Mettre en cache les chemins fréquents
3. **GraphQL** : Exposer une API GraphQL pour queries complexes

---

## Exemple Complet

### Configuration

```yaml
entities:
  - singular: company
    plural: companies
  - singular: section
    plural: sections
  - singular: employee
    plural: employees

links:
  - link_type: has_section
    source_type: company
    target_type: section
    forward_route_name: sections
    reverse_route_name: company
    auth:
      list: authenticated
      create: role:admin
  
  - link_type: has_employee
    source_type: section
    target_type: employee
    forward_route_name: employees
    reverse_route_name: section
    auth:
      list: authenticated
      create: role:manager
```

### Utilisation

```bash
# Créer les entités
POST /companies
POST /sections
POST /employees

# Créer les liens
POST /companies/{company_id}/has_section/sections/{section_id}
POST /sections/{section_id}/has_employee/employees/{employee_id}

# Naviguer (1 niveau)
GET /companies/{company_id}/sections
GET /sections/{section_id}/employees

# Naviguer (2 niveaux) - Avec traversal implémenté
GET /companies/{company_id}/sections/{section_id}/employees
```

### Vérifications de Sécurité

Le système vérifie :
1. ✅ Le lien `company → section` existe
2. ✅ Le lien `section → employee` existe
3. ✅ L'utilisateur a la permission `list` sur `has_employee`
4. ✅ Le `tenant_id` est isolé

---

## Conclusion

### ✅ Déjà Implémenté

- **Permissions au niveau des liens** : Complètement fonctionnel
- **Navigation 1 niveau** : Fonctionnel (GET /companies/{id}/sections)

### ⏳ À Implémenter

- **Path Traversal** : Pour navigation multi-niveaux
- **Middleware Auth** : Pour vérifier les permissions

### 📝 Prochaines Étapes

1. Implémenter `traverse_links()` handler
2. Ajouter les routes dans `server/router.rs`
3. Tester avec des scénarios réels
4. Optimiser les performances (si nécessaire)

---

**Voir aussi** :
- [LINK_AUTHORIZATION.md](LINK_AUTHORIZATION.md) - Documentation auth complète
- [LINK_AUTH_IMPLEMENTATION.md](../../LINK_AUTH_IMPLEMENTATION.md) - Détails techniques

