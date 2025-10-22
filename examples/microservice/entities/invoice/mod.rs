//! Invoice entity module

pub mod descriptor;
pub mod handlers;
pub mod model;
pub mod store;

pub use descriptor::InvoiceDescriptor;
pub use handlers::*;
pub use model::Invoice;
