//! Invoice entity model

use this::prelude::*;

impl_data_entity!(
    Invoice,
    "invoice",
    ["name", "number"],
    {
        number: String,
        amount: f64,
        due_date: Option<String>,
        paid_at: Option<String>,
    }
);
