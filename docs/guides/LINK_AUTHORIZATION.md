# Link Authorization Guide

## 🎯 Overview

this-rs provides **link-level authorization**, allowing you to control who can create, update, or delete links independently of entity permissions.

## 🔐 Why Link-Level Authorization?

### Problem: Entity Permissions Aren't Enough

```
Scenario: Healthcare System
- Doctors can READ patient records ✅
- Doctors can CREATE diagnoses ✅  
- But should doctors link ANY diagnosis to ANY patient? ❌
```

**Solution**: Link-level authorization lets you control the relationships themselves.

---

## 📝 Configuration

> **⚠️ Corrigé le 2026-10-01 — le schéma ci-dessous ne chargeait PAS.**
> Les valeurs d'`auth` sont des **politiques en texte** (`"authenticated"`,
> `"admin_only"`, `"role:doctor"`, `"owner"`, `"perm:…"` — le vocabulaire du
> moteur), pas des objets `{policy, roles}` : écrire l'ancienne forme faisait
> **échouer le chargement** de la configuration.
>
> Deuxième correction, de fond : **ces politiques ne sont plus à appliquer à la
> main**. Jusqu'ici elles étaient déclarées et lues par **aucun** appelant de
> production ; le middleware d'autorisation consulte désormais le type de lien
> **avant** la politique de l'entité source, quand le chemin nomme la route du
> lien (`Authorization::declared_link_policy`). Une route de lien déclarée pour
> une **autre** entité source ne décide pas ici : la résolution est celle du
> routeur — le couple `(type d'entité, nom de route)`.

### Basic Auth Configuration

```yaml
links:
  - link_type: has_diagnosis
    source_type: patient
    target_type: diagnosis
    forward_route_name: diagnoses
    reverse_route_name: patient
    auth:
      list: authenticated
      create: role:doctor        # policy strings, not objects
      update: role:doctor
      delete: admin_only         # only admins can remove diagnoses
```

### Auth Policy Types

#### 1. **Authenticated**

Any authenticated user:
```yaml
auth:
  create:
    policy: Authenticated
    roles: []
```

#### 2. **RequireRole**

User must have one of the specified roles:
```yaml
auth:
  create:
    policy: RequireRole
    roles: [doctor, nurse, admin]
```

#### 3. **AllowOwner**

User must own one of the linked entities:
```yaml
auth:
  create:
    policy: AllowOwner
    roles: [user]  # Must be a user AND own one entity
```

#### 4. **Custom**

Implement custom logic:
```yaml
auth:
  create:
    policy: CustomApprovalWorkflow
    roles: [manager]
```

---

## 🎨 Common Patterns

### Pattern 1: Different Rules per Operation

```yaml
links:
  - link_type: owner
    source_type: user
    target_type: car
    forward_route_name: cars-owned
    auth:
      create:
        policy: Authenticated    # Anyone can claim ownership
        roles: []
      delete:
        policy: AllowOwner       # Only owner can remove
        roles: []
      update:
        policy: AllowOwner       # Only owner can update metadata
        roles: []
```

### Pattern 2: Workflow-Based Authorization

```yaml
links:
  - link_type: approved_by
    source_type: document
    target_type: user
    forward_route_name: approvers
    auth:
      create:
        policy: RequireRole
        roles: [manager, director]  # Only managers can approve
      delete:
        policy: RequireRole
        roles: [admin]              # Only admins can revoke approval
```

### Pattern 3: Hierarchical Permissions

```yaml
links:
  # Regular users can add members
  - link_type: has_member
    source_type: team
    target_type: user
    forward_route_name: members
    auth:
      create:
        policy: RequireRole
        roles: [team_lead, admin]
      delete:
        policy: RequireRole
        roles: [team_lead, admin]
  
  # Only admins can assign team leads
  - link_type: has_lead
    source_type: team
    target_type: user
    forward_route_name: leads
    auth:
      create:
        policy: RequireRole
        roles: [admin]
      delete:
        policy: RequireRole
        roles: [admin]
```

---

## 💻 Implementation

### 1. Define Auth Provider

```rust
use this::prelude::*;

pub struct YourAuthProvider {
    // Your auth logic (JWT validation, DB checks, etc.)
}

#[async_trait]
impl AuthProvider for YourAuthProvider {
    async fn check_policy(
        &self,
        context: &AuthContext,
        policy: &str,
        // Additional context as needed
    ) -> Result<bool> {
        match policy {
            "Authenticated" => Ok(context.is_authenticated()),
            "RequireRole" => Ok(context.has_any_role(&required_roles)),
            "AllowOwner" => Ok(self.is_owner(context, entity_id).await?),
            _ => Ok(false),
        }
    }
}
```

### 2. Use in Handlers

> **Plus nécessaire depuis le 2026-10-01.** L'application est faite par le
> **middleware d'autorisation**, avant le handler : `LinkAuthConfig::policy_for`
> traduit l'opération (`list_links`, `create_link`, …) en politique, et la
> décision porte la portée et la provenance (`x-this-rs-authz-scope`,
> `x-this-rs-authz-policy`). Recopier cette vérification dans chaque handler
> créait un second chemin d'autorisation — la façon la plus sûre d'en laisser un
> troué. Le code ci-dessous est conservé comme **exemple d'appel direct** de la
> configuration (par exemple dans un handler maison), pas comme le chemin
> recommandé.

```rust
pub async fn create_link(
    State(state): State<AppState>,
    auth_context: AuthContext,  // Extract from request
    Path((source_type, source_id, route_name, target_id)): Path<(String, Uuid, String, Uuid)>,
    Json(payload): Json<CreateLinkRequest>,
) -> Result<Response, ExtractorError> {
    let extractor = DirectLinkExtractor::from_path(...)?;
    
    // Check authorization
    if let Some(auth_config) = &extractor.link_definition.auth {
        let policy = &auth_config.create;
        
        if !state.auth_provider.check_policy(
            &auth_context,
            &policy.policy,
        ).await? {
            return Err(ExtractorError::Unauthorized);
        }
    }
    
    // Create the link
    let link = LinkEntity::new(...);
    state.link_service.create(link).await?;
    
    Ok(...)
}
```

---

## 🧪 Testing

### Test Configuration

```rust
#[test]
fn test_auth_config_parsing() {
    let yaml = r#"
    link_type: has_invoice
    source_type: order
    target_type: invoice
    forward_route_name: invoices
    auth:
      create:
        policy: Authenticated
        roles: []
      delete:
        policy: RequireRole
        roles: [admin]
    "#;
    
    let def: LinkDefinition = serde_yaml::from_str(yaml).unwrap();
    assert!(def.auth.is_some());
}
```

### Test Runtime Authorization

```bash
# Should succeed (authenticated user)
curl -X POST http://localhost:3000/orders/123/invoices/456 \
  -H "Authorization: Bearer user-token"

# Should fail (not an admin)
curl -X DELETE http://localhost:3000/orders/123/invoices/456 \
  -H "Authorization: Bearer user-token"
# Returns: 403 Forbidden

# Should succeed (admin user)
curl -X DELETE http://localhost:3000/orders/123/invoices/456 \
  -H "Authorization: Bearer admin-token"
```

---

## 🎯 Real-World Examples

### Example 1: Document Management

```yaml
links:
  # Anyone can view document relationships
  - link_type: references
    source_type: document
    target_type: document
    forward_route_name: references
    auth:
      create:
        policy: Authenticated
        roles: []
      delete:
        policy: AllowOwner
        roles: []
  
  # Only specific roles can publish
  - link_type: published_in
    source_type: document
    target_type: collection
    forward_route_name: collections
    auth:
      create:
        policy: RequireRole
        roles: [editor, publisher, admin]
      delete:
        policy: RequireRole
        roles: [admin]
```

### Example 2: Social Network

```yaml
links:
  # Users control their own friendships
  - link_type: friend
    source_type: user
    target_type: user
    forward_route_name: friends
    auth:
      create:
        policy: AllowOwner
        roles: [user]
      delete:
        policy: AllowOwner
        roles: [user]
  
  # Moderators can block users
  - link_type: blocked
    source_type: user
    target_type: user
    forward_route_name: blocked_users
    auth:
      create:
        policy: RequireRole
        roles: [moderator, admin]
      delete:
        policy: RequireRole
        roles: [admin]
```

### Example 3: Project Management

```yaml
links:
  # Team members can assign tasks
  - link_type: assigned_to
    source_type: task
    target_type: user
    forward_route_name: assignees
    auth:
      create:
        policy: RequireRole
        roles: [team_member, project_manager]
      delete:
        policy: RequireRole
        roles: [project_manager, admin]
  
  # Only project managers can set dependencies
  - link_type: depends_on
    source_type: task
    target_type: task
    forward_route_name: dependencies
    auth:
      create:
        policy: RequireRole
        roles: [project_manager, admin]
      delete:
        policy: RequireRole
        roles: [project_manager, admin]
```

---

## 💡 Best Practices

### 1. Principle of Least Privilege

```yaml
# ✅ Good: Restrict by default, open as needed
auth:
  create:
    policy: RequireRole
    roles: [specific_role]

# ❌ Avoid: Too permissive
auth:
  create:
    policy: Authenticated
    roles: []
```

### 2. Separate Create/Delete Permissions

```yaml
# ✅ Good: Different rules for creation and deletion
auth:
  create:
    policy: Authenticated
    roles: []
  delete:
    policy: RequireRole
    roles: [admin]
```

### 3. Use Meaningful Policy Names

```yaml
# ✅ Good: Clear intent
policy: RequireManagerApproval
policy: AllowTeamMembers

# ❌ Avoid: Generic names
policy: Policy1
policy: CheckAccess
```

### 4. Document Your Policies

```yaml
# ✅ Good: Add descriptions
links:
  - link_type: approved_by
    description: "Links documents to approvers. Only managers can approve."
    auth:
      create:
        policy: RequireRole
        roles: [manager]
```

---

## 📚 Related Documentation

- [Link Auth Implementation](../architecture/LINK_AUTH_IMPLEMENTATION.md)
- [Architecture](../architecture/ARCHITECTURE.md)
- [Getting Started](GETTING_STARTED.md)

---

**Link-level authorization gives you fine-grained control over your data relationships!** 🔐🚀✨
