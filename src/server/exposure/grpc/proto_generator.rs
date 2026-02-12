//! Dynamic `.proto` file generator
//!
//! Generates a typed `.proto` definition based on the registered entity types
//! and their JSON structure. This is useful for clients that want to generate
//! typed gRPC stubs for specific deployments.
//!
//! The generated `.proto` file includes:
//! - A typed message per entity (e.g., `Order`, `Invoice`)
//! - A typed CRUD service per entity (e.g., `OrderService`, `InvoiceService`)
//! - A generic `LinkService` for relationship management

use crate::server::host::ServerHost;
use std::sync::Arc;

/// Generates typed `.proto` definitions from ServerHost entity registry
///
/// Unlike the generic `this_grpc.proto` which uses `google.protobuf.Struct`,
/// this generator creates typed messages and services specific to the deployed
/// entity types. Useful for client code generation.
pub struct ProtoGenerator {
    host: Arc<ServerHost>,
}

impl ProtoGenerator {
    /// Create a new ProtoGenerator from a ServerHost
    pub fn new(host: Arc<ServerHost>) -> Self {
        Self { host }
    }

    /// Generate the complete `.proto` file content
    pub async fn generate_proto(&self) -> String {
        let mut proto = String::new();

        // Header
        proto.push_str("syntax = \"proto3\";\n\n");
        proto.push_str("package this_api;\n\n");

        // Generate messages and services for each entity type
        for entity_type in self.host.entity_types() {
            self.generate_entity_messages(&mut proto, entity_type).await;
            self.generate_entity_service(&mut proto, entity_type);
        }

        // Generate link service
        self.generate_link_messages(&mut proto);
        self.generate_link_service(&mut proto);

        proto
    }

    /// Generate message types for a specific entity
    async fn generate_entity_messages(&self, proto: &mut String, entity_type: &str) {
        let pascal = to_pascal_case(entity_type);

        // Try to get sample entity to discover fields
        let fields = if let Some(fetcher) = self.host.entity_fetchers.get(entity_type) {
            if let Ok(sample) = fetcher.get_sample_entity().await {
                extract_fields_from_json(&sample)
            } else {
                vec![]
            }
        } else {
            vec![]
        };

        // Entity message
        proto.push_str(&format!("// {} entity\n", pascal));
        proto.push_str(&format!("message {} {{\n", pascal));
        for (i, (name, proto_type)) in fields.iter().enumerate() {
            proto.push_str(&format!("  {} {} = {};\n", proto_type, name, i + 1));
        }
        proto.push_str("}\n\n");

        // Request/Response messages
        proto.push_str(&format!("message Get{}Request {{\n", pascal));
        proto.push_str("  string id = 1;\n");
        proto.push_str("}\n\n");

        proto.push_str(&format!("message List{}Request {{\n", pascal));
        proto.push_str("  int32 limit = 1;\n");
        proto.push_str("  int32 offset = 2;\n");
        proto.push_str("}\n\n");

        proto.push_str(&format!("message List{}Response {{\n", pascal));
        proto.push_str(&format!("  repeated {} items = 1;\n", pascal));
        proto.push_str("  int32 total = 2;\n");
        proto.push_str("}\n\n");

        proto.push_str(&format!("message Create{}Request {{\n", pascal));
        // Exclude auto-generated fields
        for (i, (name, proto_type)) in fields.iter().enumerate() {
            if !is_auto_field(name) {
                proto.push_str(&format!("  {} {} = {};\n", proto_type, name, i + 1));
            }
        }
        proto.push_str("}\n\n");

        proto.push_str(&format!("message Update{}Request {{\n", pascal));
        proto.push_str("  string id = 1;\n");
        for (i, (name, proto_type)) in fields.iter().enumerate() {
            if !is_auto_field(name) {
                proto.push_str(&format!("  {} {} = {};\n", proto_type, name, i + 2));
            }
        }
        proto.push_str("}\n\n");

        proto.push_str(&format!("message Delete{}Request {{\n", pascal));
        proto.push_str("  string id = 1;\n");
        proto.push_str("}\n\n");

        proto.push_str(&format!("message Delete{}Response {{\n", pascal));
        proto.push_str("  bool success = 1;\n");
        proto.push_str("}\n\n");
    }

    /// Generate a CRUD service for an entity type
    fn generate_entity_service(&self, proto: &mut String, entity_type: &str) {
        let pascal = to_pascal_case(entity_type);

        proto.push_str(&format!("service {}Service {{\n", pascal));
        proto.push_str(&format!(
            "  rpc Get{}(Get{}Request) returns ({});\n",
            pascal, pascal, pascal
        ));
        proto.push_str(&format!(
            "  rpc List{}(List{}Request) returns (List{}Response);\n",
            pascal, pascal, pascal
        ));
        proto.push_str(&format!(
            "  rpc Create{}(Create{}Request) returns ({});\n",
            pascal, pascal, pascal
        ));
        proto.push_str(&format!(
            "  rpc Update{}(Update{}Request) returns ({});\n",
            pascal, pascal, pascal
        ));
        proto.push_str(&format!(
            "  rpc Delete{}(Delete{}Request) returns (Delete{}Response);\n",
            pascal, pascal, pascal
        ));
        proto.push_str("}\n\n");
    }

    /// Generate link-related messages
    fn generate_link_messages(&self, proto: &mut String) {
        proto.push_str("// Link messages\n");
        proto.push_str("message Link {\n");
        proto.push_str("  string id = 1;\n");
        proto.push_str("  string link_type = 2;\n");
        proto.push_str("  string source_id = 3;\n");
        proto.push_str("  string target_id = 4;\n");
        proto.push_str("  string created_at = 5;\n");
        proto.push_str("  string updated_at = 6;\n");
        proto.push_str("}\n\n");

        proto.push_str("message CreateLinkRequest {\n");
        proto.push_str("  string link_type = 1;\n");
        proto.push_str("  string source_id = 2;\n");
        proto.push_str("  string target_id = 3;\n");
        proto.push_str("}\n\n");

        proto.push_str("message GetLinkRequest {\n");
        proto.push_str("  string id = 1;\n");
        proto.push_str("}\n\n");

        proto.push_str("message FindLinksRequest {\n");
        proto.push_str("  string entity_id = 1;\n");
        proto.push_str("  string link_type = 2;\n");
        proto.push_str("}\n\n");

        proto.push_str("message LinkListResponse {\n");
        proto.push_str("  repeated Link links = 1;\n");
        proto.push_str("}\n\n");

        proto.push_str("message DeleteLinkRequest {\n");
        proto.push_str("  string id = 1;\n");
        proto.push_str("}\n\n");

        proto.push_str("message DeleteLinkResponse {\n");
        proto.push_str("  bool success = 1;\n");
        proto.push_str("}\n\n");
    }

    /// Generate the link service
    fn generate_link_service(&self, proto: &mut String) {
        proto.push_str("service LinkService {\n");
        proto.push_str("  rpc CreateLink(CreateLinkRequest) returns (Link);\n");
        proto.push_str("  rpc GetLink(GetLinkRequest) returns (Link);\n");
        proto.push_str("  rpc FindLinksBySource(FindLinksRequest) returns (LinkListResponse);\n");
        proto.push_str("  rpc FindLinksByTarget(FindLinksRequest) returns (LinkListResponse);\n");
        proto.push_str("  rpc DeleteLink(DeleteLinkRequest) returns (DeleteLinkResponse);\n");
        proto.push_str("}\n");
    }
}

/// Convert a snake_case entity type to PascalCase
fn to_pascal_case(s: &str) -> String {
    s.split('_')
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().to_string() + &chars.as_str().to_lowercase(),
            }
        })
        .collect()
}

/// Check if a field is auto-generated and shouldn't be in create/update requests
fn is_auto_field(name: &str) -> bool {
    matches!(
        name,
        "id" | "created_at" | "updated_at" | "deleted_at" | "entity_type" | "type"
    )
}

/// Extract field names and protobuf types from a JSON sample
fn extract_fields_from_json(json: &serde_json::Value) -> Vec<(String, String)> {
    let mut fields = Vec::new();

    if let serde_json::Value::Object(map) = json {
        for (key, value) in map {
            let proto_type = json_type_to_proto(value);
            fields.push((key.clone(), proto_type));
        }
    }

    fields
}

/// Map a JSON value type to a protobuf type
fn json_type_to_proto(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::Bool(_) => "bool".to_string(),
        serde_json::Value::Number(n) => {
            if n.is_f64() {
                "double".to_string()
            } else {
                "int64".to_string()
            }
        }
        serde_json::Value::String(_) => "string".to_string(),
        serde_json::Value::Array(_) => "repeated string".to_string(), // simplified
        serde_json::Value::Object(_) => "string".to_string(),         // JSON serialized
        serde_json::Value::Null => "string".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_to_pascal_case() {
        assert_eq!(to_pascal_case("order"), "Order");
        assert_eq!(to_pascal_case("user_profile"), "UserProfile");
        assert_eq!(to_pascal_case("payment_method"), "PaymentMethod");
        assert_eq!(to_pascal_case("a"), "A");
    }

    #[test]
    fn test_is_auto_field() {
        assert!(is_auto_field("id"));
        assert!(is_auto_field("created_at"));
        assert!(is_auto_field("updated_at"));
        assert!(is_auto_field("deleted_at"));
        assert!(!is_auto_field("name"));
        assert!(!is_auto_field("email"));
    }

    #[test]
    fn test_json_type_to_proto() {
        use serde_json::json;
        assert_eq!(json_type_to_proto(&json!(true)), "bool");
        assert_eq!(json_type_to_proto(&json!(42.5)), "double");
        assert_eq!(json_type_to_proto(&json!("hello")), "string");
        assert_eq!(json_type_to_proto(&json!(null)), "string");
        assert_eq!(json_type_to_proto(&json!(["a", "b"])), "repeated string");
    }

    #[test]
    fn test_extract_fields_from_json() {
        use serde_json::json;
        let sample = json!({
            "id": "uuid-here",
            "name": "Test",
            "amount": 42.5,
            "active": true
        });

        let fields = extract_fields_from_json(&sample);
        assert!(!fields.is_empty());

        // Check that all fields are present
        let field_names: Vec<&str> = fields.iter().map(|(n, _)| n.as_str()).collect();
        assert!(field_names.contains(&"id"));
        assert!(field_names.contains(&"name"));
        assert!(field_names.contains(&"amount"));
        assert!(field_names.contains(&"active"));
    }
}
