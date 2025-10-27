//! Payment entity model with validation and filtering

use this::prelude::*;

impl_data_entity_validated!(
    Payment,
    "payment",
    ["name", "number"],
    {
        number: String,
        amount: f64,
        method: String,
        transaction_id: Option<String>,
    },
    validate: {
        create: {
            number: [required string_length(3, 20)],
            amount: [required positive max_value(1_000_000.0)],
            method: [required string_length(3, 50)],
        },
        update: {
            amount: [optional positive max_value(1_000_000.0)],
            method: [optional string_length(3, 50)],
        },
    },
    filters: {
        create: {
            number: [trim uppercase],
            method: [trim lowercase],
            amount: [round_decimals(2)],
        },
        update: {
            method: [trim lowercase],
            amount: [round_decimals(2)],
        },
    }
);
