//! Payment entity model

use this::prelude::*;

impl_data_entity!(
    Payment,
    "payment",
    ["name", "number"],
    {
        number: String,
        amount: f64,
        method: String,
        transaction_id: Option<String>,
    }
);
