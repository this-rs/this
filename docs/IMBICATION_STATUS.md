# État de l'Imbrication à 3+ Niveaux

## ✅ Ce Qui Fonctionne

Le framework **DÉTECTE automatiquement** les chaînes de liens et les **AFFICHE** :
```
🔗 Chaînes de liens détectées dans la configuration:
   ✅ Route imbriquée générée: /orders/{order_id}/invoices/{invoice_id}/payments
   📊 1 routes imbriquées à 3+ niveaux ajoutées automatiquement
```

## ❌ Ce Qui Ne Fonctionne Pas

Les routes à 3+ niveaux **CRÉENT une erreur "Invalid entity ID format"** car :

1. **Le parsing des segments échoue** - `RecursiveLinkExtractor` s'attend à un pattern `type/id/route` mais reçoit un chemin avec des paramètres Axum
2. **Les handlers imbriqués ne parviennent pas à extraire les vrais paramètres** - Axum utilise `{param}` mais le handler ne peut pas les décoder

## 🔧 Pourquoi C'est Complexe

L'imbrication automatique à 3+ niveaux nécessiterait :
1. Parser dynamiquement des chemins comme `/orders/UUID/invoices/UUID/payments`  
2. Extraire les UUIDs intermédiaires
3. Reconstruire les segments pour le `RecursiveLinkExtractor`
4. Valider que tous les liens existent dans la DB

C'est techniquement possible mais **très complexe** à faire de manière générique.

## 💡 Solution Recommandée

**Utilisez les routes à 2 niveaux** qui fonctionnent parfaitement :
```bash
GET /orders/{order_id}/invoices          # Récupérer invoices
GET /invoices/{invoice_id}/payments      # Récupérer payments
```

C'est plus RESTful et plus simple que des chemins très longs !

