# Full API Example

## Description

Exemple complet avec serveur HTTP Axum démontrant :
- Configuration YAML des entités et liens
- Routes HTTP automatiques pour la navigation des liens
- Registry de routes pour résolution bidirectionnelle
- Serveur HTTP complet avec Axum

## Structure

```
full_api/
└── main.rs    # Serveur HTTP complet
```

## Exécution

```bash
cargo run --example full_api
```

Le serveur démarre sur `http://127.0.0.1:3000`

## Routes Disponibles

### Navigation de Liens (Semantic URLs)
- `GET /:entity_type/:entity_id/:route_name` - Liste les entités liées
- `GET /:source_type/:source_id/:route_name/:target_id` - Récupère un lien spécifique
- `POST /:source_type/:source_id/:route_name/:target_id` - Crée un lien
- `PUT /:source_type/:source_id/:route_name/:target_id` - Met à jour la metadata
- `DELETE /:source_type/:source_id/:route_name/:target_id` - Supprime un lien
- `GET /:entity_type/:entity_id/links` - Introspection (découvre tous les liens)

## Exemples de Requêtes

```bash
# Liste les voitures possédées par un user
curl -H 'X-Tenant-ID: <TENANT_ID>' \
  http://127.0.0.1:3000/users/<USER_ID>/cars-owned

# Liste les users qui possèdent une voiture
curl -H 'X-Tenant-ID: <TENANT_ID>' \
  http://127.0.0.1:3000/cars/<CAR_ID>/users-owners

# Introspection
curl -H 'X-Tenant-ID: <TENANT_ID>' \
  http://127.0.0.1:3000/users/<USER_ID>/links
```

## Ce Que Vous Apprendrez

- ✅ Configuration YAML des entités et liens
- ✅ Serveur HTTP avec Axum
- ✅ Routes génériques auto-générées
- ✅ Navigation bidirectionnelle via HTTP
- ✅ Registry de routes
- ✅ Introspection d'API

