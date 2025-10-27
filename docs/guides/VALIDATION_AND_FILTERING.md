# Automatic Validation and Filtering

## Overview

This guide explains how to use the automatic validation and filtering system built into the framework. The system automatically applies validators and filters **before** your handlers receive the data, ensuring that data is always clean and valid.

## Architecture

The system consists of:

1. **Validators** - Reusable functions that validate fields
2. **Filters** - Reusable functions that transform values
3. **Macro `impl_data_entity_validated!`** - Declarative definition in `model.rs`
4. **Extractor `Validated<T>`** - Automatic extraction in handlers

## 🚀 Quick Start

### 1. Define Entity with Validation

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
    
    // Validation rules by operation
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
    
    // Filters by operation
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

### 2. Use in Handlers

```rust
// entities/invoice/handlers.rs
use this::prelude::Validated;

pub async fn create_invoice(
    State(state): State<InvoiceAppState>,
    validated: Validated<Invoice>,  // ← Automatic validation!
) -> Result<Json<Invoice>, StatusCode> {
    // Data is already filtered and validated!
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

## 📋 Available Validators

### `required`
Checks that the field is not null.

```rust
number: [required]
```

### `optional`
Marks the field as optional (always valid).

```rust
due_date: [optional]
```

### `positive`
Checks that the number is positive (> 0).

```rust
amount: [positive]
```

### `string_length(min, max)`
Checks the length of a string.

```rust
number: [string_length(3, 50)]
```

### `max_value(max)`
Checks that the number does not exceed a maximum value.

```rust
amount: [max_value(1_000_000.0)]
```

### `in_list("val1", "val2", ...)`
Checks that the value is in an allowed list.

```rust
status: [in_list("draft", "sent", "paid", "cancelled")]
```

### `date_format(format)`
Checks that a date matches the specified format.

```rust
due_date: [date_format("%Y-%m-%d")]
```

## 🔧 Available Filters

### `trim`
Removes spaces at the beginning and end of a string.

```rust
number: [trim]
```

### `uppercase`
Converts a string to uppercase.

```rust
number: [uppercase]
```

### `lowercase`
Converts a string to lowercase.

```rust
status: [lowercase]
```

### `round_decimals(decimals)`
Rounds a number to the specified number of decimals.

```rust
amount: [round_decimals(2)]
```

## 🎯 Usage Examples

### Example 1: Basic Validation

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

### Example 2: Chaining Validators and Filters

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

### Example 3: Different Validation by Operation

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

## ⚙️ Internal Functioning

### 1. Execution Order

When a request arrives:

1. **JSON Extraction** : JSON is parsed
2. **Operation Determination** : Based on HTTP method (POST = create, PUT = update)
3. **Filter Application** : Transformations are applied
4. **Validator Application** : Validations are executed
5. **Handler** : The handler receives clean and validated data

### 2. Error Handling

If validation fails, an HTTP 422 response is returned with details:

```json
{
  "error": "Validation failed",
  "errors": [
    "Field 'amount' must be positive (value: -100)",
    "'status' must be one of: [\"draft\", \"sent\", \"paid\"] (current value: invalid)"
  ]
}
```

### 3. Extensibility

#### Create a Custom Validator

```rust
// src/core/validation/validators.rs

pub fn email_format() -> impl Fn(&str, &Value) -> Result<(), String> + Send + Sync + Clone {
    |field: &str, value: &Value| {
        if let Some(s) = value.as_str() {
            if s.contains('@') && s.contains('.') {
                Ok(())
            } else {
                Err(format!("'{}' must be a valid email address", field))
            }
        } else {
            Ok(())
        }
    }
}
```

Then add it to the macro helper:

```rust
// src/entities/macros.rs - add_validators_for_field!

($config:expr, $field:expr, email_format $( $rest:tt )*) => {
    $config.add_validator($field, $crate::core::validation::validators::email_format());
    $crate::add_validators_for_field!($config, $field, $( $rest )*);
};
```

#### Create a Custom Filter

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

### Enable Logs

```rust
// In main.rs
tracing_subscriber::fmt()
    .with_max_level(tracing::Level::DEBUG)
    .init();
```

### Manual Testing

```bash
# Test with invalid data
curl -X POST http://127.0.0.1:3000/invoices \
  -H "Content-Type: application/json" \
  -d '{"number": "  inv-test  ", "status": " DRAFT ", "amount": 1234.567}'

# Expected result:
# - number: "INV-TEST" (trimmed and uppercased)
# - status: "draft" (trimmed and lowercased)
# - amount: 1234.57 (rounded to 2 decimals)
```

## ✅ Best Practices

1. **Separate validation and filtering** : Filters transform, validators verify
2. **Use optional for optional fields** : Avoids false positives
3. **Order logically** : Trim before length validation
4. **Operation-specific validations** : Create can be stricter than Update
5. **Clear error messages** : Validators include the problematic value

## 📚 Resources

- [Validators source](../../src/core/validation/validators.rs)
- [Filters source](../../src/core/validation/filters.rs)
- [Macro implementation](../../src/entities/macros.rs)
- [Microservice example](../../examples/microservice/)

## 🎉 Conclusion

The automatic validation and filtering system allows you to:

- ✅ Declare your rules directly in `model.rs`
- ✅ Guarantee that handlers always receive valid data
- ✅ Reuse validators/filters across all your entities
- ✅ Maintain clean and maintainable code
- ✅ Have detailed error messages automatically

**The system is 100% integrated with the framework and follows its declarative philosophy!**
