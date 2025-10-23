//! Order entity model

use this::prelude::*;

impl_data_entity!(
    Order,
    "order",
    ["name", "number", "customer_name"],
    {
        number: String,
        amount: f64,
        customer_name: Option<String>,
        notes: Option<String>,
    }
);
