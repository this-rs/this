//! Order entity model with validation and filtering

use this::prelude::*;

impl_data_entity_validated!(
    Order,
    "order",
    ["name", "number", "customer_name"],
    {
        number: String,
        amount: f64,
        customer_name: Option<String>,
        notes: Option<String>,
    },
    validate: {
        create: {
            number: [required string_length(3, 20)],
            amount: [required positive max_value(1_000_000.0)],
            status: [required in_list("pending", "confirmed", "shipped", "delivered", "cancelled")],
        },
        update: {
            amount: [optional positive max_value(1_000_000.0)],
            status: [optional in_list("pending", "confirmed", "shipped", "delivered", "cancelled")],
        },
    },
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
