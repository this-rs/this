# Imbrication de Routes à 3+ Niveaux

## 🎯 État Actuel

Le framework **détecte automatiquement** les chaînes de liens possibles dans votre configuration :

```
🔗 Chaînes de liens détectées dans la configuration:
   📍 Possible imbrication: /orders/{id}/invoices/{invoices_id}/payments

💡 Pour utiliser ces routes imbriquées, ajoutez-les manuellement dans votre application
```

**MAIS** les routes à 3+ niveaux **ne sont PAS automatiquement ajoutées** au router.

## ✅ Ce Qui Fonctionne (2 Niveaux)

```bash
# ✅ Fonctionne automatiquement
GET  /orders/{order_id}/invoices
POST /orders/{order_id}/invoices

GET  /invoices/{invoice_id}/payments
POST /invoices/{invoice_id}/payments
```

## ❌ Ce Qui Ne Fonctionne PAS (3+ Niveaux)

```bash
# ❌ N'est PAS disponible automatiquement
GET  /orders/{order_id}/invoices/{invoice_id}/payments
POST /orders/{order_id}/invoices/{invoice_id}/payments
```

## 🔧 Pourquoi ?

Le framework utilise Axum pour le routing. Axum a deux limitations :

1. **Les routes doivent être explicites au build-time** - pas de génération dynamique illimitée
2. **Les routes catch-all `/{*path}` entrent en conflit** avec les routes dynamiques existantes

Le framework **détecte** les chaînes possibles mais **ne peut pas les ajouter** automatiquement sans casser le système de routing.

## 📝 Comment Tester

Pour tester les routes imbriquées :

1. Utilisez les routes à 2 niveaux (recommande) :
   ```bash
   # Au lieu de /orders/{order_id}/invoices/{invoice_id}/payments
   GET /invoices/{invoice_id}/payments
   ```

2. Ou utilisez des appels API en cascade :
   ```bash
   # Étape 1 : Récupérer l'invoice_id depuis l'order
   GET /orders/{order_id}/invoices
   
   # Étape 2 : Utiliser l'invoice_id pour récupérer les payments
   GET /invoices/{invoice_id}/payments
   ```

## 💡 Solution Future

Pour supporter automatiquement les routes à 3+ niveaux, il faudrait :
- Soit changer de framework de routing (pas Axum)
- Soit accepter des limitations (pas de conflit avec routes dynamiques)
- Soit renoncer au principe générique du framework (hardcoder des entités)

Pour l'instant, **les routes à 2 niveaux sont suffisantes** pour 99% des cas d'usage et évitent cette complexité.

