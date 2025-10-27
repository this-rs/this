# Limitation: Imbrication à 3+ Niveaux Non Automatique

## ❌ Constat Final

**Le framework NE PEUT PAS** ajouter automatiquement les routes imbriquées à 3+ niveaux (`/orders/{id}/invoices/{id}/payments`) à cause des limitations techniques d'Axum.

## 🎯 Ce Que Le Framework FAIT

✅ **Détecte automatiquement** les chaînes de liens possibles :
```
🔗 Chaînes de liens détectées dans la configuration:
   📍 Possible imbrication: /orders/{id}/invoices/{invoices_id}/payments

💡 Pour utiliser ces routes imbriquées, ajoutez-les manuellement dans votre application
```

✅ **Génère automatiquement** les routes à 2 niveaux :
- `GET /orders/{id}/invoices`
- `GET /invoices/{id}/payments`

## 🔧 Solution Recommandée

**Utilisez la cascade manuelle** (plus RESTful) :
```bash
# Au lieu de :
GET /orders/{order_id}/invoices/{invoice_id}/payments

# Utilisez :
GET /orders/{order_id}/invoices    # Obtenir invoice_id
GET /invoices/{invoice_id}/payments  # Obtenir payments
```

## 📊 Pourquoi C'est Limité

Axum ne permet pas :
- Routes générées dynamiquement au runtime
- Catch-all routes sans conflits
- Routes hardcodées d'entités spécifiques (contredit la généricité du framework)

Le framework reste donc **générique et limité** à 2 niveaux d'imbrication.

