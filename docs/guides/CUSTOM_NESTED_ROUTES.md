# Ajouter des Routes Imbriquées à 3+ Niveaux

## 🎯 Objectif

Le framework supporte nativement les routes imbriquées à **2 niveaux** :
- `GET /{entity_type}/{entity_id}/{route_name}`

Si vous souhaitez des **imbrications plus profondes** (3+ niveaux), vous pouvez ajouter vos propres routes spécifiques dans votre application.

## 📝 Exemple avec Order → Invoice → Payment

Dans votre fichier `main.rs` de votre application, après avoir créé le router principal avec `ServerBuilder`, vous pouvez ajouter des routes personnalisées :

```rust
use this::links::handlers::{AppState, handle_nested_path_get, handle_nested_path_post};

#[tokio::main]
async fn main() -> Result<()> {
    // ... votre configuration existante ...
    
    let app = ServerBuilder::new()
        .with_link_service(link_service_arc.clone())
        .register_module(module)?
        .build()?;
    
    // Ajouter des routes imbriquées personnalisées
    // Note: Ces routes doivent être AVANT les routes génériques pour avoir priorité
    let app = Router::new()
        // Routes imbriquées à 3 niveaux
        .route(
            "/orders/:order_id/invoices/:invoice_id/payments",
            get(|State(state): State<AppState>, path: Path<String>| async move {
                handle_nested_path_get(State(state), path).await
            }).post(|State(state): State<AppState>, path: Path<String>, Json(payload): Json<CreateLinkedEntityRequest>| async move {
                handle_nested_path_post(State(state), path, Json(payload)).await
            })
        )
        .route(
            "/orders/:order_id/invoices/:invoice_id/payments/:payment_id",
            get(|State(state): State<AppState>, path: Path<String>| async move {
                handle_nested_path_get(State(state), path).await
            })
        )
        // Merge avec votre app principale
        .merge(app);
    
    // ... reste du code ...
}
```

## ⚠️ Note Importante

Les handlers `handle_nested_path_get` et `handle_nested_path_post` acceptent un `Path<String>` qui doit contenir **tout le chemin après le premier slash**, pas les segments individuels.

Par exemple, pour `/orders/123/invoices/456/payments`, le `Path<String>` devra contenir `orders/123/invoices/456/payments`.

## 🔧 Alternative : Utiliser les Routes Existantes

Sauf besoin spécifique, vous pouvez utiliser les routes à 2 niveaux existantes :

```bash
# Au lieu de faire :
GET /orders/{order_id}/invoices/{invoice_id}/payments

# Vous pouvez faire :
GET /invoices/{invoice_id}/payments
```

Cela évite d'avoir à créer des routes spécifiques et garde votre application simple.

## 🚀 Limitation Actuelle

Le framework ne génère **pas automatiquement** les routes imbriquées à 3+ niveaux pour éviter :
- Des conflits avec les routes dynamiques existantes
- Des routes inutiles si pas utilisées
- De la complexité pour des cas peu courants

Les routes à **2 niveaux** restent suffisantes pour la plupart des cas d'usage.

