//! DynamoDB implementation of DataService and LinkService

use crate::core::{Data, DataService, link::LinkEntity, LinkService};
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
                _ => {
                    // Skip complex types for now
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
