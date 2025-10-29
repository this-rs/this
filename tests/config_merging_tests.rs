//! Integration tests for config merging functionality

use this::prelude::*;

#[test]
fn test_merge_empty_configs() {
    let merged = LinksConfig::merge(vec![]);

    assert_eq!(merged.entities.len(), 0);
    assert_eq!(merged.links.len(), 0);
    assert!(merged.validation_rules.is_none());
}

#[test]
fn test_merge_single_config() {
    let config = LinksConfig::default_config();
    let original_entities = config.entities.len();
    let original_links = config.links.len();

    let merged = LinksConfig::merge(vec![config]);

    assert_eq!(merged.entities.len(), original_entities);
    assert_eq!(merged.links.len(), original_links);
}

#[test]
fn test_merge_multiple_configs_no_overlap() {
    let config1_yaml = r#"
entities:
  - singular: order
    plural: orders
  - singular: invoice
    plural: invoices

links:
  - link_type: generates
    source_type: order
    target_type: invoice
    forward_route_name: invoices
    reverse_route_name: order
"#;

    let config2_yaml = r#"
entities:
  - singular: payment
    plural: payments
  - singular: refund
    plural: refunds

links:
  - link_type: settles
    source_type: payment
    target_type: invoice
    forward_route_name: settled-invoices
    reverse_route_name: payments
"#;

    let config1 = LinksConfig::from_yaml_str(config1_yaml).unwrap();
    let config2 = LinksConfig::from_yaml_str(config2_yaml).unwrap();

    let merged = LinksConfig::merge(vec![config1, config2]);

    // Should have all 4 entities
    assert_eq!(merged.entities.len(), 4);

    // Should have both links
    assert_eq!(merged.links.len(), 2);

    // Verify entity names
    let entity_names: Vec<String> = merged.entities.iter().map(|e| e.singular.clone()).collect();
    assert!(entity_names.contains(&"order".to_string()));
    assert!(entity_names.contains(&"invoice".to_string()));
    assert!(entity_names.contains(&"payment".to_string()));
    assert!(entity_names.contains(&"refund".to_string()));
}

#[test]
fn test_merge_configs_with_entity_overlap() {
    let config1_yaml = r#"
entities:
  - singular: user
    plural: users
    auth:
      list: public
      create: admin_only

links: []
"#;

    let config2_yaml = r#"
entities:
  - singular: user
    plural: users
    auth:
      list: authenticated
      create: authenticated

links: []
"#;

    let config1 = LinksConfig::from_yaml_str(config1_yaml).unwrap();
    let config2 = LinksConfig::from_yaml_str(config2_yaml).unwrap();

    let merged = LinksConfig::merge(vec![config1, config2]);

    // Should have only 1 user entity (last one wins)
    assert_eq!(merged.entities.len(), 1);

    let user_entity = &merged.entities[0];
    assert_eq!(user_entity.singular, "user");

    // Should use config2's auth (last wins)
    assert_eq!(user_entity.auth.list, "authenticated");
    assert_eq!(user_entity.auth.create, "authenticated");
}

#[test]
fn test_merge_configs_with_link_overlap() {
    let config1_yaml = r#"
entities:
  - singular: order
    plural: orders
  - singular: invoice
    plural: invoices

links:
  - link_type: generates
    source_type: order
    target_type: invoice
    forward_route_name: invoices
    reverse_route_name: order
    description: "Order generates invoice (v1)"
"#;

    let config2_yaml = r#"
entities:
  - singular: order
    plural: orders
  - singular: invoice
    plural: invoices

links:
  - link_type: generates
    source_type: order
    target_type: invoice
    forward_route_name: invoices-generated
    reverse_route_name: order-source
    description: "Order generates invoice (v2)"
"#;

    let config1 = LinksConfig::from_yaml_str(config1_yaml).unwrap();
    let config2 = LinksConfig::from_yaml_str(config2_yaml).unwrap();

    let merged = LinksConfig::merge(vec![config1, config2]);

    // Should have only 1 link (last one wins)
    assert_eq!(merged.links.len(), 1);

    let link = &merged.links[0];
    assert_eq!(link.link_type, "generates");
    assert_eq!(link.forward_route_name, "invoices-generated"); // From config2
    assert_eq!(
        link.description.as_ref().unwrap(),
        "Order generates invoice (v2)"
    );
}

#[test]
fn test_merge_configs_with_validation_rules() {
    let config1_yaml = r#"
entities:
  - singular: user
    plural: users
  - singular: company
    plural: companies

links:
  - link_type: works_at
    source_type: user
    target_type: company
    forward_route_name: companies
    reverse_route_name: employees

validation_rules:
  works_at:
    - source: user
      targets: [company]
"#;

    let config2_yaml = r#"
entities:
  - singular: project
    plural: projects

links:
  - link_type: works_at
    source_type: user
    target_type: project
    forward_route_name: projects
    reverse_route_name: contributors

validation_rules:
  works_at:
    - source: user
      targets: [project]
"#;

    let config1 = LinksConfig::from_yaml_str(config1_yaml).unwrap();
    let config2 = LinksConfig::from_yaml_str(config2_yaml).unwrap();

    let merged = LinksConfig::merge(vec![config1, config2]);

    // Validation rules should be combined
    assert!(merged.validation_rules.is_some());
    let rules = merged.validation_rules.unwrap();

    // Should have rules for works_at from both configs
    assert!(rules.contains_key("works_at"));
    let works_at_rules = &rules["works_at"];
    assert_eq!(works_at_rules.len(), 2); // Both rules combined
}

#[test]
fn test_merge_three_configs() {
    let config1_yaml = r#"
entities:
  - singular: order
    plural: orders

links:
  - link_type: has_item
    source_type: order
    target_type: product
    forward_route_name: products
    reverse_route_name: orders
"#;

    let config2_yaml = r#"
entities:
  - singular: invoice
    plural: invoices

links:
  - link_type: generated_from
    source_type: invoice
    target_type: order
    forward_route_name: order
    reverse_route_name: invoices
"#;

    let config3_yaml = r#"
entities:
  - singular: payment
    plural: payments

links:
  - link_type: settles
    source_type: payment
    target_type: invoice
    forward_route_name: invoice
    reverse_route_name: payments
"#;

    let config1 = LinksConfig::from_yaml_str(config1_yaml).unwrap();
    let config2 = LinksConfig::from_yaml_str(config2_yaml).unwrap();
    let config3 = LinksConfig::from_yaml_str(config3_yaml).unwrap();

    let merged = LinksConfig::merge(vec![config1, config2, config3]);

    // Should have all 3 entities
    assert_eq!(merged.entities.len(), 3);

    // Should have all 3 links
    assert_eq!(merged.links.len(), 3);

    // Verify link types
    let link_types: Vec<String> = merged.links.iter().map(|l| l.link_type.clone()).collect();
    assert!(link_types.contains(&"has_item".to_string()));
    assert!(link_types.contains(&"generated_from".to_string()));
    assert!(link_types.contains(&"settles".to_string()));
}

#[test]
fn test_merge_preserves_entity_auth_config() {
    let config_yaml = r#"
entities:
  - singular: order
    plural: orders
    auth:
      list: authenticated
      create: service_only
      delete: admin_only

links: []
"#;

    let config = LinksConfig::from_yaml_str(config_yaml).unwrap();
    let merged = LinksConfig::merge(vec![config]);

    assert_eq!(merged.entities.len(), 1);

    let order_entity = &merged.entities[0];
    assert_eq!(order_entity.auth.list, "authenticated");
    assert_eq!(order_entity.auth.create, "service_only");
    assert_eq!(order_entity.auth.delete, "admin_only");
}

#[test]
fn test_merge_with_complex_scenario() {
    // Module 1: Catalog service
    let catalog_yaml = r#"
entities:
  - singular: product
    plural: products
  - singular: category
    plural: categories

links:
  - link_type: belongs_to
    source_type: product
    target_type: category
    forward_route_name: category
    reverse_route_name: products
"#;

    // Module 2: Order service
    let order_yaml = r#"
entities:
  - singular: order
    plural: orders
  - singular: product
    plural: products
    auth:
      list: public

links:
  - link_type: contains
    source_type: order
    target_type: product
    forward_route_name: products
    reverse_route_name: orders
"#;

    // Module 3: Billing service
    let billing_yaml = r#"
entities:
  - singular: invoice
    plural: invoices
  - singular: payment
    plural: payments

links:
  - link_type: generates
    source_type: order
    target_type: invoice
    forward_route_name: invoices
    reverse_route_name: order
  - link_type: settles
    source_type: payment
    target_type: invoice
    forward_route_name: invoice
    reverse_route_name: payments
"#;

    let catalog_config = LinksConfig::from_yaml_str(catalog_yaml).unwrap();
    let order_config = LinksConfig::from_yaml_str(order_yaml).unwrap();
    let billing_config = LinksConfig::from_yaml_str(billing_yaml).unwrap();

    let merged = LinksConfig::merge(vec![catalog_config, order_config, billing_config]);

    // Entities: product (overridden by order module), category, order, invoice, payment = 5
    assert_eq!(merged.entities.len(), 5);

    // Links: belongs_to, contains, generates, settles = 4
    assert_eq!(merged.links.len(), 4);

    // Verify product entity uses last definition (from order module with public auth)
    let product = merged
        .entities
        .iter()
        .find(|e| e.singular == "product")
        .unwrap();
    assert_eq!(product.auth.list, "public");
}
