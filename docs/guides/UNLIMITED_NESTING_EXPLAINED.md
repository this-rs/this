# Pourquoi l'Imbrication Illimitée Ne Peut Pas Être Automatique

## 🎯 Objectif du Problème

Vous souhaitiez que le framework génère automatiquement des routes à profondeur illimitée comme :
- `GET /users/{id}/invoices/{id}/orders`
- `GET /users/{id}/invoices/{id}/orders/{id}`

## ❌ Pourquoi C'est Impossible avec Axum

### Le Problème Fondamental

Les routes imbriquées à **3+ niveaux** ne peuvent **PAS** être générées automatiquement par le framework car :

1. **Axum requiert des routes explicites au build-time** - pas de génération dynamique
2. **Une route catch-all `/{*path}` entre en conflit** avec les routes dynamiques existantes
3. **Le framework doit rester générique** - il ne peut pas hardcoder des entités spécifiques comme "orders", "invoices", etc.

### Que Fait Le Framework

Le framework **détecte automatiquement** les chaînes de liens possibles dans votre configuration :

```
🔗 Chaînes de liens détectées dans la configuration:
   📍 Possible imbrication: /orders/{id}/invoices/{invoices_id}/payments

💡 Pour utiliser ces routes imbriquées, ajoutez-les manuellement dans votre application
```

Vous devez **ajouter ces routes manuellement** dans votre application, pas dans le framework.

## ✅ Solutions Disponibles

### Option 1 : Routes à 2 Niveaux (Recommandé)

Utilisez les routes existantes. Au lieu de :
```
GET /orders/{order_id}/invoices/{invoice_id}/payments
```

Faites plutôt :
```
GET /invoices/{invoice_id}/payments
```

**Avantages** :
- ✅ Plus simple
- ✅ Fonctionne déjà
- ✅ Évite les chemins trop longs
- ✅ Plus RESTful

### Option 2 : Routes Imbriquées Custom dans Votre App

Si vous avez vraiment besoin de 3+ niveaux dans votre application, ajoutez-les manuellement :

```rust
use this::links::handlers::{handle_nested_path_get, handle_nested_path_post};
use axum::{Router, routing::{get, post}};

let app = ServerBuilder::new()
    .with_link_service(link_service)
    .register_module(module)?
    .build()?;

// Ajouter vos routes custom
let nested_router = Router::new()
    .route("/orders/:order_id/invoices/:invoice_id/payments", 
        get(|| async { /* ... */ })
        .post(|| async { /* ... */ })
    )
    .with_state(state);

let final_app = app.merge(nested_router);
```

## 🔧 Code Disponible dans le Framework

Même si l'imbrication illimitée n'est pas automatique, le framework fournit :

### Extracteurs Récursifs

```rust
use this::core::extractors::RecursiveLinkExtractor;

// Parser n'importe quel chemin imbriqué
let extractor = RecursiveLinkExtractor::from_segments(
    vec!["users", "123", "invoices", "456", "orders"],
    &registry,
    &config
)?;
```

### Handlers Génériques

```rust
use this::links::handlers::{handle_nested_path_get, handle_nested_path_post};

// Ces handlers fonctionnent pour n'importe quel niveau d'imbrication
```

## 📊 Recommandation Finale

Pour 99% des cas d'usage, les **routes à 2 niveaux** sont suffisantes :

```bash
# Au lieu de 3+ niveaux
GET /users/123/orders/456/invoices/789/payments

# Utilisez 2 niveaux
GET /invoices/789/payments
```

Si vous avez absolument besoin de plus, implémentez-le dans **votre application**, pas dans le framework.

## 🎯 Conclusion

Le framework reste **générique et agnostique** aux entités. Si l'imbrication illimitée devenait automatique, il faudrait :
1. Soit accepter des routes hardcodées (contradiction)
2. Soit utiliser une syntaxe de routing différente d'Axum
3. Soit risquer des conflits de routing

**Solution actuelle** : Routes à 2 niveaux + outils disponibles pour custom si besoin.

