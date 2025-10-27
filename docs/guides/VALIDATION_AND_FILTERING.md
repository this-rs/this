# Validation et Filtrage Automatique

## Vue d'ensemble

Ce guide explique comment utiliser le système de validation et de filtrage automatique intégré dans le framework. Le système applique automatiquement les validators et les filters **avant** que vos handlers ne reçoivent les données, garantissant ainsi que les données sont toujours propres et valides.

## Architecture

Le système est composé de :

1. **Validators** - Fonctions réutilisables qui valident des champs
2. **Filters** - Fonctions réutilisables qui transforment des valeurs
3. **Macro `impl_data_entity_validated!`** - Définition déclarative dans `model.rs`
4. **Extractor `Validated<T>`** - Extraction automatique dans les handlers

## 🚀 Quick Start

### 1. Définir l'entité avec validation

```rust
// entities/invoice/model.rs
use this::prelude::*;

impl_data_entity_validated!(
    Invoice,
    "invoice",
    ["name", "number"],
    {
        number: String,
        amount: f64,
        due_date: Option<String>,
        paid_at: Option<String>,
    },
    
    // Validation rules par opération
    validate: {
        create: {
            number: [required string_length(3, 50)],
            amount: [required positive max_value(1_000_000.0)],
            status: [required in_list("draft", "sent", "paid", "cancelled")],
            due_date: [optional date_format("%Y-%m-%d")],
        },
        update: {
            amount: [optional positive max_value(1_000_000.0)],
            status: [optional in_list("draft", "sent", "paid", "cancelled")],
        },
    },
    
    // Filters par opération
    filters: {
        create: {
            number: [trim uppercase],
            status: [trim lowercase],
            amount: [round_decimals(2)],
        },
        update: {
            status: [trim lowercase],
            amount: [round_decimals(2)],
        },
    }
);
```

### 2. Utiliser dans les handlers

```rust
// entities/invoice/handlers.rs
use this::prelude::Validated;

pub async fn create_invoice(
    State(state): State<InvoiceAppState>,
    validated: Validated<Invoice>,  // ← Validation automatique !
) -> Result<Json<Invoice>, StatusCode> {
    // Les données sont déjà filtrées et validées !
    let payload = &*validated;
    
    let invoice = Invoice::new(
        payload["number"].as_str().unwrap().to_string(),
        payload["status"].as_str().unwrap().to_string(),
        payload["number"].as_str().unwrap().to_string(),
        payload["amount"].as_f64().unwrap(),
        payload["due_date"].as_str().map(String::from),
        payload["paid_at"].as_str().map(String::from),
    );

    state.store.add(invoice.clone());
    Ok(Json(invoice))
}
```

## 📋 Validators Disponibles

### `required`
Vérifie que le champ n'est pas null.

```rust
number: [required]
```

### `optional`
Marque le champ comme optionnel (toujours valide).

```rust
due_date: [optional]
```

### `positive`
Vérifie que le nombre est positif (> 0).

```rust
amount: [positive]
```

### `string_length(min, max)`
Vérifie la longueur d'une chaîne.

```rust
number: [string_length(3, 50)]
```

### `max_value(max)`
Vérifie que le nombre ne dépasse pas une valeur maximale.

```rust
amount: [max_value(1_000_000.0)]
```

### `in_list("val1", "val2", ...)`
Vérifie que la valeur est dans une liste autorisée.

```rust
status: [in_list("draft", "sent", "paid", "cancelled")]
```

### `date_format(format)`
Vérifie qu'une date correspond au format spécifié.

```rust
due_date: [date_format("%Y-%m-%d")]
```

## 🔧 Filters Disponibles

### `trim`
Supprime les espaces au début et à la fin d'une chaîne.

```rust
number: [trim]
```

### `uppercase`
Convertit une chaîne en majuscules.

```rust
number: [uppercase]
```

### `lowercase`
Convertit une chaîne en minuscules.

```rust
status: [lowercase]
```

### `round_decimals(decimals)`
Arrondit un nombre au nombre de décimales spécifié.

```rust
amount: [round_decimals(2)]
```

## 🎯 Exemples d'Usage

### Exemple 1: Validation de base

```rust
impl_data_entity_validated!(
    User,
    "user",
    ["name", "email"],
    {
        email: String,
        age: u32,
    },
    validate: {
        create: {
            name: [required string_length(2, 100)],
            email: [required],
            age: [required positive],
        },
    },
    filters: {
        create: {
            name: [trim],
            email: [trim lowercase],
        },
    }
);
```

### Exemple 2: Chaînage de validators et filters

```rust
impl_data_entity_validated!(
    Product,
    "product",
    ["name", "sku"],
    {
        sku: String,
        price: f64,
        category: String,
    },
    validate: {
        create: {
            sku: [required string_length(5, 20)],
            price: [required positive max_value(999999.99)],
            category: [required in_list("electronics", "clothing", "food")],
        },
    },
    filters: {
        create: {
            sku: [trim uppercase],
            price: [round_decimals(2)],
            category: [trim lowercase],
        },
    }
);
```

### Exemple 3: Validation différente par opération

```rust
impl_data_entity_validated!(
    Order,
    "order",
    ["number", "status"],
    {
        number: String,
        amount: f64,
        notes: Option<String>,
    },
    validate: {
        create: {
            number: [required string_length(5, 30)],
            amount: [required positive],
        },
        update: {
            amount: [optional positive],
            notes: [optional string_length(0, 500)],
        },
    },
    filters: {
        create: {
            number: [trim uppercase],
            amount: [round_decimals(2)],
        },
        update: {
            amount: [round_decimals(2)],
            notes: [trim],
        },
    }
);
```

## ⚙️ Fonctionnement Interne

### 1. Ordre d'exécution

Lorsqu'une requête arrive :

1. **Extraction JSON** : Le JSON est parsé
2. **Détermination de l'opération** : Basée sur la méthode HTTP (POST = create, PUT = update)
3. **Application des filters** : Les transformations sont appliquées
4. **Application des validators** : Les validations sont exécutées
5. **Handler** : Le handler reçoit les données propres et validées

### 2. Gestion des erreurs

Si la validation échoue, une réponse HTTP 422 est retournée avec les détails :

```json
{
  "error": "Validation failed",
  "errors": [
    "Le champ 'amount' doit être positif (valeur: -100)",
    "'status' doit être l'une des valeurs: [\"draft\", \"sent\", \"paid\"] (valeur actuelle: invalid)"
  ]
}
```

### 3. Extensibilité

#### Créer un validator personnalisé

```rust
// src/core/validation/validators.rs

pub fn email_format() -> impl Fn(&str, &Value) -> Result<(), String> + Send + Sync + Clone {
    |field: &str, value: &Value| {
        if let Some(s) = value.as_str() {
            if s.contains('@') && s.contains('.') {
                Ok(())
            } else {
                Err(format!("'{}' doit être une adresse email valide", field))
            }
        } else {
            Ok(())
        }
    }
}
```

Puis ajoutez-le à la macro helper :

```rust
// src/entities/macros.rs - add_validators_for_field!

($config:expr, $field:expr, email_format $( $rest:tt )*) => {
    $config.add_validator($field, $crate::core::validation::validators::email_format());
    $crate::add_validators_for_field!($config, $field, $( $rest )*);
};
```

#### Créer un filter personnalisé

```rust
// src/core/validation/filters.rs

pub fn slugify() -> impl Fn(&str, Value) -> Result<Value> + Send + Sync + Clone {
    |_: &str, value: Value| {
        if let Some(s) = value.as_str() {
            let slug = s.to_lowercase()
                .chars()
                .map(|c| if c.is_alphanumeric() || c == '-' { c } else { '-' })
                .collect::<String>();
            Ok(Value::String(slug))
        } else {
            Ok(value)
        }
    }
}
```

## 🔍 Debugging

### Activer les logs

```rust
// Dans main.rs
tracing_subscriber::fmt()
    .with_max_level(tracing::Level::DEBUG)
    .init();
```

### Tester manuellement

```bash
# Test avec des données invalides
curl -X POST http://127.0.0.1:3000/invoices \
  -H "Content-Type: application/json" \
  -d '{"number": "  inv-test  ", "status": " DRAFT ", "amount": 1234.567}'

# Résultat attendu:
# - number: "INV-TEST" (trimé et uppercasé)
# - status: "draft" (trimé et lowercasé)
# - amount: 1234.57 (arrondi à 2 décimales)
```

## ✅ Best Practices

1. **Séparez validation et filtrage** : Les filters transforment, les validators vérifient
2. **Utilisez optional pour les champs optionnels** : Évite les faux positifs
3. **Ordonnez logiquement** : Trim avant validation de longueur
4. **Validations spécifiques par opération** : Create peut être plus strict qu'Update
5. **Messages d'erreur clairs** : Les validators incluent la valeur problématique

## 📚 Ressources

- [Validators source](../../src/core/validation/validators.rs)
- [Filters source](../../src/core/validation/filters.rs)
- [Macro implementation](../../src/entities/macros.rs)
- [Exemple microservice](../../examples/microservice/)

## 🎉 Conclusion

Le système de validation et filtrage automatique vous permet de :

- ✅ Déclarer vos règles directement dans `model.rs`
- ✅ Garantir que les handlers reçoivent toujours des données valides
- ✅ Réutiliser des validators/filters à travers toutes vos entités
- ✅ Maintenir un code propre et maintenable
- ✅ Avoir des messages d'erreur détaillés automatiquement

**Le système est 100% intégré au framework et suit sa philosophie déclarative !**

