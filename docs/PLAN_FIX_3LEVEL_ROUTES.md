# Plan pour Réparer les Routes à 3+ Niveaux

## 🎯 Objectif
Faire fonctionner `GET /orders/{order_id}/invoices/{invoice_id}/payments`

## 🔍 Diagnostic du Problème

Le problème est dans le **flux de données** :

1. ✅ Route générée : `/orders/{order_id}/invoices/{invoice_id}/payments`
2. ✅ Route matche correctement
3. ✅ Handler est appelé
4. ❌ Handler reçoit le chemin et essaie de parser avec `RecursiveLinkExtractor`
5. ❌ Le parser s'attend à `orders/UUID/invoices/UUID/payments` mais le format ne correspond pas

## 🛠️ Solution : Handler Direct avec Extraction Manuelle

Au lieu d'utiliser `RecursiveLinkExtractor` (qui est trop complexe), créons un **handler direct** qui :

1. **Extrait explicitement** les paramètres Axum : `order_id`, `invoice_id`
2. **Valide que** l'invoice appartient bien à l'order
3. **Appelle directement** le LinkService pour trouver les payments
4. **Retourne les données**

## 📝 Implémentation

### Étape 1 : Handler Spécialisé pour 3+ Niveaux

Créer `src/links/handlers.rs::handle_3level_route` qui :
- Extrait `order_id`, `invoice_id` directement
- Vérifie que `order → invoice` existe
- Récupère `payments` pour cette invoice
- Retourne le résultat enrichi

### Étape 2 : Builder Dynamique

Dans `build_nested_link_routes`, pour chaque chaîne détectée :
- Créer un handler qui extrait TOUS les params de la chaîne
- Appeler `handle_3level_route` avec ces params

### Étape 3 : Test

Tester que `/orders/{order_id}/invoices/{invoice_id}/payments` retourne bien les payments.

