//! Invoice entity model with validation and filtering

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

    // Validation rules per operation
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
            due_date: [optional date_format("%Y-%m-%d")],
        },
    },

    // Filters per operation
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
