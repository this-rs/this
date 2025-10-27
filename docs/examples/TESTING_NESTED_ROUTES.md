# Comment Tester les Routes Imbriquées

## ✅ Routes à 2 Niveaux (Fonctionnent)

### Test 1 : Récupérer les invoices d'un order
```bash
# Récupérer tous les orders
curl http://127.0.0.1:3000/orders | jq .

# Récupérer les invoices d'un order spécifique
curl http://127.0.0.1:3000/orders/{ORDER_ID}/invoices | jq .
```

### Test 2 : Récupérer les payments d'une invoice
```bash
# Récupérer toutes les invoices
curl http://127.0.0.1:3000/invoices | jq .

# Récupérer les payments d'une invoice spécifique  
curl http://127.0.0.1:3000/invoices/{INVOICE_ID}/payments | jq .
```

### Test 3 : Cascade Manuelle (Équivalent à 3+ niveaux)

```bash
# Étape 1 : Récupérer l'ID de la première invoice de l'order
ORDER_ID="e0f301b0-88cc-432c-a98a-721e1baa5180"
INVOICE_ID=$(curl -s "http://127.0.0.1:3000/orders/${ORDER_ID}/invoices" | jq -r '.links[0].target_id')

# Étape 2 : Utiliser cet invoice_id pour récupérer les payments
curl "http://127.0.0.1:3000/invoices/${INVOICE_ID}/payments" | jq .
```

## ❌ Routes à 3+ Niveaux (NON Disponibles)

```bash
# ❌ Ces routes ne fonctionnent PAS
curl http://127.0.0.1:3000/orders/{ORDER_ID}/invoices/{INVOICE_ID}/payments

# Erreur 404 - route non trouvée
```

## 🎯 Pourquoi ?

Le framework détecte les chaînes possibles mais ne peut pas les ajouter automatiquement à cause des limitations d'Axum (voir `docs/guides/UNLIMITED_NESTING_EXPLAINED.md`).

## ✅ Solution Recommandée

Utilisez la cascade manuelle :
```bash
# 1. Get order invoices
curl http://127.0.0.1:3000/orders/{id}/invoices

# 2. For each invoice, get payments  
curl http://127.0.0.1:3000/invoices/{id}/payments
```

C'est plus RESTful et plus simple que des chemins très longs !

