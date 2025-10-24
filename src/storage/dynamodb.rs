//! DynamoDB implementation of DataService and LinkService

use crate::core::{link::LinkEntity, Data, DataService, LinkService};
use anyhow::Result;
use async_trait::async_trait;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_dynamodb::Client as DynamoDBClient;
use std::collections::HashMap;
use uuid::Uuid;

/// DynamoDB implementation of DataService
#[derive(Clone)]
pub struct DynamoDBDataService<T: Data + serde::Serialize + for<'de> serde::Deserialize<'de>> {
    client: DynamoDBClient,
    table_name: String,
    _phantom: std::marker::PhantomData<T>,
}

impl<T: Data + serde::Serialize + for<'de> serde::Deserialize<'de>> DynamoDBDataService<T> {
    pub fn new(client: DynamoDBClient, table_name: String) -> Self {
        Self {
            client,
            table_name,
            _phantom: std::marker::PhantomData,
        }
    }

    /// List entities by tenant ID using DynamoDB Query (efficient for multi-tenant tables)
    ///
    /// This method uses Query instead of Scan, which is much more efficient when
    /// `tenant_id` is the partition key or part of a GSI.
    ///
    /// # Arguments
    /// * `tenant_id` - The tenant ID to query for
    ///
    /// # Table Structure Requirements
    /// This method assumes one of the following table structures:
    /// - Partition key: `tenant_id`, Sort key: `id` (or any other)
    /// - A Global Secondary Index (GSI) with `tenant_id` as partition key
    ///
    /// # Example
    /// ```rust,ignore
    /// let service = DynamoDBDataService::new(client, "users".to_string());
    /// let tenant_id = Uuid::parse_str("...")?;
    /// let users = service.list_by_tenant(&tenant_id).await?;
    /// ```
    pub async fn list_by_tenant(&self, tenant_id: &Uuid) -> Result<Vec<T>>
    where
        T: Data + Send + Sync + 'static,
    {
        let result = self
            .client
            .query()
            .table_name(&self.table_name)
            .key_condition_expression("tenant_id = :tenant_id")
            .expression_attribute_values(":tenant_id", AttributeValue::S(tenant_id.to_string()))
            .send()
            .await?;

        let mut entities = Vec::new();
        if let Some(items) = result.items {
            for item in items {
                entities.push(self.item_to_entity(&item).await?);
            }
        }
        Ok(entities)
    }

    /// Get an entity by tenant_id and id (for tables with composite keys)
    ///
    /// This method is used when the table has a composite primary key:
    /// - Partition key: `tenant_id`
    /// - Sort key: `id`
    ///
    /// # Example
    /// ```rust,ignore
    /// let service = DynamoDBDataService::new(client, "tenants".to_string());
    /// let tenant_id = Uuid::parse_str("...")?;
    /// let id = Uuid::parse_str("...")?;
    /// let tenant = service.get_with_tenant(&tenant_id, &id).await?;
    /// ```
    pub async fn get_with_tenant(&self, tenant_id: &Uuid, id: &Uuid) -> Result<Option<T>>
    where
        T: Data + Send + Sync + 'static,
    {
        let key = HashMap::from([
            (
                "tenant_id".to_string(),
                AttributeValue::S(tenant_id.to_string()),
            ),
            ("id".to_string(), AttributeValue::S(id.to_string())),
        ]);

        let result = self
            .client
            .get_item()
            .table_name(&self.table_name)
            .set_key(Some(key))
            .send()
            .await?;

        match result.item() {
            Some(item) => Ok(Some(self.item_to_entity(item).await?)),
            None => Ok(None),
        }
    }

    /// List entities by tenant ID using a specific GSI (Global Secondary Index)
    ///
    /// Use this method when `tenant_id` is indexed via a GSI rather than being
    /// the primary partition key.
    ///
    /// # Arguments
    /// * `tenant_id` - The tenant ID to query for
    /// * `index_name` - Name of the GSI with tenant_id as partition key
    ///
    /// # Example
    /// ```rust,ignore
    /// let service = DynamoDBDataService::new(client, "users".to_string());
    /// let tenant_id = Uuid::parse_str("...")?;
    /// let users = service.list_by_tenant_gsi(&tenant_id, "tenant_id-index").await?;
    /// ```
    pub async fn list_by_tenant_gsi(&self, tenant_id: &Uuid, index_name: &str) -> Result<Vec<T>>
    where
        T: Data + Send + Sync + 'static,
    {
        let result = self
            .client
            .query()
            .table_name(&self.table_name)
            .index_name(index_name)
            .key_condition_expression("tenant_id = :tenant_id")
            .expression_attribute_values(":tenant_id", AttributeValue::S(tenant_id.to_string()))
            .send()
            .await?;

        let mut entities = Vec::new();
        if let Some(items) = result.items {
            for item in items {
                entities.push(self.item_to_entity(&item).await?);
            }
        }
        Ok(entities)
    }

    async fn entity_to_item(&self, entity: &T) -> Result<HashMap<String, AttributeValue>> {
        // Convert entity to JSON first, then to DynamoDB format
        let json = serde_json::to_value(entity)?;
        let mut item = HashMap::new();

        // Add basic fields
        if let Some(id) = json.get("id").and_then(|v| v.as_str()) {
            item.insert("id".to_string(), AttributeValue::S(id.to_string()));
        }

        // Add all other fields
        for (key, value) in json.as_object().unwrap_or(&serde_json::Map::new()) {
            if key != "id" {
                if let Some(str_val) = value.as_str() {
                    item.insert(key.clone(), AttributeValue::S(str_val.to_string()));
                } else if let Some(num_val) = value.as_f64() {
                    item.insert(key.clone(), AttributeValue::N(num_val.to_string()));
                } else if let Some(bool_val) = value.as_bool() {
                    item.insert(key.clone(), AttributeValue::Bool(bool_val));
                } else if let Some(arr) = value.as_array() {
                    // Handle arrays by converting to DynamoDB List
                    let list_items: Vec<AttributeValue> = arr
                        .iter()
                        .filter_map(|v| {
                            if let Some(s) = v.as_str() {
                                Some(AttributeValue::S(s.to_string()))
                            } else if let Some(n) = v.as_f64() {
                                Some(AttributeValue::N(n.to_string()))
                            } else {
                                v.as_bool().map(AttributeValue::Bool)
                            }
                        })
                        .collect();
                    if !list_items.is_empty() {
                        item.insert(key.clone(), AttributeValue::L(list_items));
                    }
                } else if value.is_null() {
                    // Skip null values
                } else {
                    // For complex types (objects, etc.), serialize as JSON string
                    item.insert(key.clone(), AttributeValue::S(value.to_string()));
                }
            }
        }

        Ok(item)
    }

    async fn item_to_entity(&self, item: &HashMap<String, AttributeValue>) -> Result<T> {
        // Convert from DynamoDB format to JSON
        let mut json = serde_json::Map::new();

        for (key, value) in item {
            match value {
                AttributeValue::S(s) => {
                    json.insert(key.clone(), serde_json::Value::String(s.clone()));
                }
                AttributeValue::N(n) => {
                    if let Ok(num) = n.parse::<f64>() {
                        json.insert(
                            key.clone(),
                            serde_json::Value::Number(serde_json::Number::from_f64(num).unwrap()),
                        );
                    }
                }
                AttributeValue::Bool(b) => {
                    json.insert(key.clone(), serde_json::Value::Bool(*b));
                }
                AttributeValue::L(list) => {
                    // Handle DynamoDB lists by converting to JSON array
                    let json_array: Vec<serde_json::Value> = list
                        .iter()
                        .filter_map(|item| match item {
                            AttributeValue::S(s) => Some(serde_json::Value::String(s.clone())),
                            AttributeValue::N(n) => n
                                .parse::<f64>()
                                .ok()
                                .and_then(serde_json::Number::from_f64)
                                .map(serde_json::Value::Number),
                            AttributeValue::Bool(b) => Some(serde_json::Value::Bool(*b)),
                            _ => None,
                        })
                        .collect();
                    json.insert(key.clone(), serde_json::Value::Array(json_array));
                }
                _ => {
                    // Skip other complex types for now
                }
            }
        }

        Ok(serde_json::from_value(serde_json::Value::Object(json))?)
    }
}

#[async_trait]
impl<T: Data + serde::Serialize + for<'de> serde::Deserialize<'de>> DataService<T>
    for DynamoDBDataService<T>
{
    async fn create(&self, entity: T) -> Result<T> {
        let item = self.entity_to_item(&entity).await?;

        self.client
            .put_item()
            .table_name(&self.table_name)
            .set_item(Some(item))
            .send()
            .await?;

        Ok(entity)
    }

    async fn get(&self, id: &Uuid) -> Result<Option<T>> {
        let key = HashMap::from([("id".to_string(), AttributeValue::S(id.to_string()))]);

        let result = self
            .client
            .get_item()
            .table_name(&self.table_name)
            .set_key(Some(key))
            .send()
            .await?;

        match result.item() {
            Some(item) => Ok(Some(self.item_to_entity(item).await?)),
            None => Ok(None),
        }
    }

    async fn list(&self) -> Result<Vec<T>> {
        let result = self
            .client
            .scan()
            .table_name(&self.table_name)
            .send()
            .await?;

        let mut entities = Vec::new();
        if let Some(items) = result.items {
            for item in items {
                entities.push(self.item_to_entity(&item).await?);
            }
        }
        Ok(entities)
    }

    async fn update(&self, _id: &Uuid, entity: T) -> Result<T> {
        let item = self.entity_to_item(&entity).await?;

        self.client
            .put_item()
            .table_name(&self.table_name)
            .set_item(Some(item))
            .send()
            .await?;

        Ok(entity)
    }

    async fn delete(&self, id: &Uuid) -> Result<()> {
        let key = HashMap::from([("id".to_string(), AttributeValue::S(id.to_string()))]);

        self.client
            .delete_item()
            .table_name(&self.table_name)
            .set_key(Some(key))
            .send()
            .await?;

        Ok(())
    }

    async fn search(&self, field: &str, value: &str) -> Result<Vec<T>> {
        // Use scan with filter for general search
        let result = self
            .client
            .scan()
            .table_name(&self.table_name)
            .filter_expression(format!("{} = :value", field))
            .expression_attribute_values(":value", AttributeValue::S(value.to_string()))
            .send()
            .await?;

        let mut entities = Vec::new();
        if let Some(items) = result.items {
            for item in items {
                entities.push(self.item_to_entity(&item).await?);
            }
        }
        Ok(entities)
    }
}

/// DynamoDB implementation of LinkService
pub struct DynamoDBLinkService {
    client: DynamoDBClient,
    table_name: String,
}

impl DynamoDBLinkService {
    pub fn new(client: DynamoDBClient, table_name: String) -> Self {
        Self { client, table_name }
    }

    /// List links by tenant ID using DynamoDB Query (efficient for multi-tenant link tables)
    ///
    /// This method uses Query instead of Scan, which is much more efficient when
    /// `tenant_id` is the partition key of the links table.
    ///
    /// # Arguments
    /// * `tenant_id` - The tenant ID to query for
    ///
    /// # Table Structure Requirements
    /// - Partition key: `tenant_id`, Sort key: `id` (recommended for multi-tenant)
    /// - Or a GSI with `tenant_id` as partition key (use `list_links_by_tenant_gsi` instead)
    ///
    /// # Example
    /// ```rust,ignore
    /// let service = DynamoDBLinkService::new(client, "links".to_string());
    /// let tenant_id = Uuid::parse_str("...")?;
    /// let links = service.list_links_by_tenant(&tenant_id).await?;
    /// ```
    pub async fn list_links_by_tenant(&self, tenant_id: &Uuid) -> Result<Vec<LinkEntity>> {
        let result = self
            .client
            .query()
            .table_name(&self.table_name)
            .key_condition_expression("tenant_id = :tenant_id")
            .expression_attribute_values(":tenant_id", AttributeValue::S(tenant_id.to_string()))
            .send()
            .await?;

        let mut links = Vec::new();
        if let Some(items) = result.items {
            for item in items {
                links.push(self.item_to_link(&item).await?);
            }
        }
        Ok(links)
    }

    /// List links by tenant ID using a specific GSI
    ///
    /// # Arguments
    /// * `tenant_id` - The tenant ID to query for
    /// * `index_name` - Name of the GSI with tenant_id as partition key
    pub async fn list_links_by_tenant_gsi(
        &self,
        tenant_id: &Uuid,
        index_name: &str,
    ) -> Result<Vec<LinkEntity>> {
        let result = self
            .client
            .query()
            .table_name(&self.table_name)
            .index_name(index_name)
            .key_condition_expression("tenant_id = :tenant_id")
            .expression_attribute_values(":tenant_id", AttributeValue::S(tenant_id.to_string()))
            .send()
            .await?;

        let mut links = Vec::new();
        if let Some(items) = result.items {
            for item in items {
                links.push(self.item_to_link(&item).await?);
            }
        }
        Ok(links)
    }

    /// Find links by source entity within a specific tenant (using Query)
    ///
    /// More efficient than `find_by_source` when used in multi-tenant context.
    /// Requires a composite GSI with (tenant_id, source_id) or table structure
    /// that supports this query pattern.
    pub async fn find_by_source_and_tenant(
        &self,
        tenant_id: &Uuid,
        source_id: &Uuid,
        link_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        // Build the filter expression for link_type if provided
        let mut query = self
            .client
            .query()
            .table_name(&self.table_name)
            .key_condition_expression("tenant_id = :tenant_id")
            .expression_attribute_values(":tenant_id", AttributeValue::S(tenant_id.to_string()));

        // Add filter for source_id
        let mut filter_parts = vec!["source_id = :source_id".to_string()];
        query = query
            .expression_attribute_values(":source_id", AttributeValue::S(source_id.to_string()));

        // Add optional link_type filter
        if let Some(lt) = link_type {
            filter_parts.push("link_type = :link_type".to_string());
            query =
                query.expression_attribute_values(":link_type", AttributeValue::S(lt.to_string()));
        }

        let filter_expression = filter_parts.join(" AND ");
        let result = query.filter_expression(filter_expression).send().await?;

        let mut links = Vec::new();
        if let Some(items) = result.items {
            for item in items {
                links.push(self.item_to_link(&item).await?);
            }
        }
        Ok(links)
    }

    async fn link_to_item(&self, link: &LinkEntity) -> Result<HashMap<String, AttributeValue>> {
        // Convert link to JSON first, then to DynamoDB format
        let json = serde_json::to_value(link)?;
        let mut item = HashMap::new();

        // Add all fields
        for (key, value) in json.as_object().unwrap_or(&serde_json::Map::new()) {
            match value {
                serde_json::Value::String(s) => {
                    item.insert(key.clone(), AttributeValue::S(s.clone()));
                }
                serde_json::Value::Number(n) => {
                    if let Some(num) = n.as_f64() {
                        item.insert(key.clone(), AttributeValue::N(num.to_string()));
                    }
                }
                serde_json::Value::Bool(b) => {
                    item.insert(key.clone(), AttributeValue::Bool(*b));
                }
                serde_json::Value::Null => {
                    // Skip null values
                }
                _ => {
                    // For complex types, serialize as JSON string
                    item.insert(key.clone(), AttributeValue::S(value.to_string()));
                }
            }
        }

        Ok(item)
    }

    async fn item_to_link(&self, item: &HashMap<String, AttributeValue>) -> Result<LinkEntity> {
        // Convert from DynamoDB format to JSON
        let mut json = serde_json::Map::new();

        for (key, value) in item {
            match value {
                AttributeValue::S(s) => {
                    // Try to parse as JSON for nested objects
                    if key == "metadata" {
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
                            json.insert(key.clone(), parsed);
                        } else {
                            json.insert(key.clone(), serde_json::Value::String(s.clone()));
                        }
                    } else {
                        json.insert(key.clone(), serde_json::Value::String(s.clone()));
                    }
                }
                AttributeValue::N(n) => {
                    if let Ok(num) = n.parse::<f64>() {
                        json.insert(
                            key.clone(),
                            serde_json::Value::Number(serde_json::Number::from_f64(num).unwrap()),
                        );
                    }
                }
                AttributeValue::Bool(b) => {
                    json.insert(key.clone(), serde_json::Value::Bool(*b));
                }
                _ => {
                    // Skip complex types
                }
            }
        }

        Ok(serde_json::from_value(serde_json::Value::Object(json))?)
    }
}

#[async_trait]
impl LinkService for DynamoDBLinkService {
    async fn create(&self, link: LinkEntity) -> Result<LinkEntity> {
        let item = self.link_to_item(&link).await?;

        self.client
            .put_item()
            .table_name(&self.table_name)
            .set_item(Some(item))
            .send()
            .await?;

        Ok(link)
    }

    async fn get(&self, id: &Uuid) -> Result<Option<LinkEntity>> {
        let key = HashMap::from([("id".to_string(), AttributeValue::S(id.to_string()))]);

        let result = self
            .client
            .get_item()
            .table_name(&self.table_name)
            .set_key(Some(key))
            .send()
            .await?;

        match result.item() {
            Some(item) => Ok(Some(self.item_to_link(item).await?)),
            None => Ok(None),
        }
    }

    async fn list(&self) -> Result<Vec<LinkEntity>> {
        let result = self
            .client
            .scan()
            .table_name(&self.table_name)
            .send()
            .await?;

        let mut links = Vec::new();
        if let Some(items) = result.items {
            for item in items {
                links.push(self.item_to_link(&item).await?);
            }
        }
        Ok(links)
    }

    async fn find_by_source(
        &self,
        source_id: &Uuid,
        link_type: Option<&str>,
        _target_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        // Use scan with filter
        let mut filter_expr = "source_id = :source_id".to_string();
        let mut attr_values = HashMap::new();
        attr_values.insert(
            ":source_id".to_string(),
            AttributeValue::S(source_id.to_string()),
        );

        if let Some(lt) = link_type {
            filter_expr.push_str(" AND link_type = :link_type");
            attr_values.insert(":link_type".to_string(), AttributeValue::S(lt.to_string()));
        }

        let mut scan = self
            .client
            .scan()
            .table_name(&self.table_name)
            .filter_expression(filter_expr);

        for (key, value) in attr_values {
            scan = scan.expression_attribute_values(key, value);
        }

        let result = scan.send().await?;

        let mut links = Vec::new();
        if let Some(items) = result.items {
            for item in items {
                links.push(self.item_to_link(&item).await?);
            }
        }
        Ok(links)
    }

    async fn find_by_target(
        &self,
        target_id: &Uuid,
        link_type: Option<&str>,
        _source_type: Option<&str>,
    ) -> Result<Vec<LinkEntity>> {
        // Use scan with filter
        let mut filter_expr = "target_id = :target_id".to_string();
        let mut attr_values = HashMap::new();
        attr_values.insert(
            ":target_id".to_string(),
            AttributeValue::S(target_id.to_string()),
        );

        if let Some(lt) = link_type {
            filter_expr.push_str(" AND link_type = :link_type");
            attr_values.insert(":link_type".to_string(), AttributeValue::S(lt.to_string()));
        }

        let mut scan = self
            .client
            .scan()
            .table_name(&self.table_name)
            .filter_expression(filter_expr);

        for (key, value) in attr_values {
            scan = scan.expression_attribute_values(key, value);
        }

        let result = scan.send().await?;

        let mut links = Vec::new();
        if let Some(items) = result.items {
            for item in items {
                links.push(self.item_to_link(&item).await?);
            }
        }
        Ok(links)
    }

    async fn update(&self, id: &Uuid, updated_link: LinkEntity) -> Result<LinkEntity> {
        // Verify the link exists first
        self.get(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Link not found"))?;

        // Save the updated link
        let item = self.link_to_item(&updated_link).await?;
        self.client
            .put_item()
            .table_name(&self.table_name)
            .set_item(Some(item))
            .send()
            .await?;

        Ok(updated_link)
    }

    async fn delete(&self, id: &Uuid) -> Result<()> {
        let key = HashMap::from([("id".to_string(), AttributeValue::S(id.to_string()))]);

        self.client
            .delete_item()
            .table_name(&self.table_name)
            .set_key(Some(key))
            .send()
            .await?;

        Ok(())
    }

    async fn delete_by_entity(&self, entity_id: &Uuid) -> Result<()> {
        // Find all links involving this entity (as source or target)
        let source_links = self.find_by_source(entity_id, None, None).await?;
        let target_links = self.find_by_target(entity_id, None, None).await?;

        // Delete all found links
        for link in source_links.into_iter().chain(target_links.into_iter()) {
            self.delete(&link.id).await?;
        }

        Ok(())
    }
}
