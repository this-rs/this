//! DynamoDB implementation of DataService and LinkService

use crate::core::{Data, DataService, EntityReference, Link, LinkService};
use anyhow::Result;
use async_trait::async_trait;
use aws_sdk_dynamodb::Client as DynamoDBClient;
use aws_sdk_dynamodb::types::AttributeValue;
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
        // Simple implementation - convert to JSON first, then to DynamoDB format
        let json = serde_json::to_value(entity)?;
        let mut item = HashMap::new();
        
        // Add basic fields
        if let Some(id) = json.get("id").and_then(|v| v.as_str()) {
            item.insert("id".to_string(), AttributeValue::S(id.to_string()));
        }
        if let Some(tenant_id) = json.get("tenant_id").and_then(|v| v.as_str()) {
            item.insert("tenant_id".to_string(), AttributeValue::S(tenant_id.to_string()));
        }
        
        // Add other fields as strings for now (simplified)
        for (key, value) in json.as_object().unwrap_or(&serde_json::Map::new()) {
            if key != "id" && key != "tenant_id" {
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
        // Simple implementation - convert from DynamoDB format to JSON
        let mut json = serde_json::Map::new();
        
        for (key, value) in item {
            match value {
                AttributeValue::S(s) => {
                    json.insert(key.clone(), serde_json::Value::String(s.clone()));
                }
                AttributeValue::N(n) => {
                    if let Ok(num) = n.parse::<f64>() {
                        json.insert(key.clone(), serde_json::Value::Number(serde_json::Number::from_f64(num).unwrap()));
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
impl<T: Data + serde::Serialize + for<'de> serde::Deserialize<'de>> DataService<T> for DynamoDBDataService<T> {
    async fn create(&self, tenant_id: &Uuid, entity: T) -> Result<T> {
        let mut item = self.entity_to_item(&entity).await?;
        
        // Add tenant_id to the item
        item.insert("tenant_id".to_string(), 
                   AttributeValue::S(tenant_id.to_string()));
        
        self.client
            .put_item()
            .table_name(&self.table_name)
            .set_item(Some(item))
            .send()
            .await?;
            
        Ok(entity)
    }

    async fn get(&self, tenant_id: &Uuid, id: &Uuid) -> Result<Option<T>> {
        let key = HashMap::from([
            ("tenant_id".to_string(), AttributeValue::S(tenant_id.to_string())),
            ("id".to_string(), AttributeValue::S(id.to_string())),
        ]);

        let result = self.client
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

    async fn list(&self, tenant_id: &Uuid) -> Result<Vec<T>> {
        let result = self.client
            .query()
            .table_name(&self.table_name)
            .key_condition_expression("tenant_id = :tenant_id")
            .expression_attribute_values(":tenant_id", 
                                       AttributeValue::S(tenant_id.to_string()))
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

    async fn update(&self, tenant_id: &Uuid, _id: &Uuid, entity: T) -> Result<T> {
        let mut item = self.entity_to_item(&entity).await?;
        item.insert("tenant_id".to_string(), 
                   AttributeValue::S(tenant_id.to_string()));
        
        self.client
            .put_item()
            .table_name(&self.table_name)
            .set_item(Some(item))
            .send()
            .await?;
            
        Ok(entity)
    }

    async fn delete(&self, tenant_id: &Uuid, id: &Uuid) -> Result<()> {
        let key = HashMap::from([
            ("tenant_id".to_string(), AttributeValue::S(tenant_id.to_string())),
            ("id".to_string(), AttributeValue::S(id.to_string())),
        ]);

        self.client
            .delete_item()
            .table_name(&self.table_name)
            .set_key(Some(key))
            .send()
            .await?;
            
        Ok(())
    }

    async fn search(&self, tenant_id: &Uuid, field: &str, value: &str) -> Result<Vec<T>> {
        // Use scan with filter for general search
        let result = self.client
            .scan()
            .table_name(&self.table_name)
            .filter_expression(format!("tenant_id = :tenant_id AND {} = :value", field))
            .expression_attribute_values(":tenant_id", 
                                       AttributeValue::S(tenant_id.to_string()))
            .expression_attribute_values(":value", 
                                       AttributeValue::S(value.to_string()))
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

    async fn link_to_item(&self, link: &Link) -> Result<HashMap<String, AttributeValue>> {
        // Simple implementation - convert to JSON first, then to DynamoDB format
        let json = serde_json::to_value(link)?;
        let mut item = HashMap::new();
        
        // Add basic fields
        if let Some(id) = json.get("id").and_then(|v| v.as_str()) {
            item.insert("id".to_string(), AttributeValue::S(id.to_string()));
        }
        if let Some(tenant_id) = json.get("tenant_id").and_then(|v| v.as_str()) {
            item.insert("tenant_id".to_string(), AttributeValue::S(tenant_id.to_string()));
        }
        if let Some(link_type) = json.get("link_type").and_then(|v| v.as_str()) {
            item.insert("link_type".to_string(), AttributeValue::S(link_type.to_string()));
        }
        
        // Add source and target as JSON strings
        if let Some(source) = json.get("source") {
            item.insert("source".to_string(), AttributeValue::S(source.to_string()));
        }
        if let Some(target) = json.get("target") {
            item.insert("target".to_string(), AttributeValue::S(target.to_string()));
        }
        
        // Add timestamps
        if let Some(created_at) = json.get("created_at").and_then(|v| v.as_str()) {
            item.insert("created_at".to_string(), AttributeValue::S(created_at.to_string()));
        }
        if let Some(updated_at) = json.get("updated_at").and_then(|v| v.as_str()) {
            item.insert("updated_at".to_string(), AttributeValue::S(updated_at.to_string()));
        }
        
        // Add metadata if present
        if let Some(metadata) = json.get("metadata") {
            if !metadata.is_null() {
                item.insert("metadata".to_string(), AttributeValue::S(metadata.to_string()));
            }
        }
        
        Ok(item)
    }

    async fn item_to_link(&self, item: &HashMap<String, AttributeValue>) -> Result<Link> {
        // Simple implementation - convert from DynamoDB format to JSON
        let mut json = serde_json::Map::new();
        
        for (key, value) in item {
            match value {
                AttributeValue::S(s) => {
                    if key == "source" || key == "target" || key == "metadata" {
                        // Parse nested JSON
                        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(s) {
                            json.insert(key.clone(), parsed);
                        }
                    } else {
                        json.insert(key.clone(), serde_json::Value::String(s.clone()));
                    }
                }
                AttributeValue::N(n) => {
                    if let Ok(num) = n.parse::<f64>() {
                        json.insert(key.clone(), serde_json::Value::Number(serde_json::Number::from_f64(num).unwrap()));
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
impl LinkService for DynamoDBLinkService {
    async fn create(
        &self,
        tenant_id: &Uuid,
        link_type: &str,
        source: EntityReference,
        target: EntityReference,
        metadata: Option<serde_json::Value>,
    ) -> Result<Link> {
        let link = Link::new(*tenant_id, link_type, source, target, metadata);
        let item = self.link_to_item(&link).await?;
        
        self.client
            .put_item()
            .table_name(&self.table_name)
            .set_item(Some(item))
            .send()
            .await?;
            
        Ok(link)
    }

    async fn get(&self, tenant_id: &Uuid, id: &Uuid) -> Result<Option<Link>> {
        let key = HashMap::from([
            ("tenant_id".to_string(), AttributeValue::S(tenant_id.to_string())),
            ("id".to_string(), AttributeValue::S(id.to_string())),
        ]);

        let result = self.client
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

    async fn list(&self, tenant_id: &Uuid) -> Result<Vec<Link>> {
        let result = self.client
            .query()
            .table_name(&self.table_name)
            .key_condition_expression("tenant_id = :tenant_id")
            .expression_attribute_values(":tenant_id", 
                                       AttributeValue::S(tenant_id.to_string()))
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
        tenant_id: &Uuid,
        source_id: &Uuid,
        source_type: &str,
        link_type: Option<&str>,
        target_type: Option<&str>,
    ) -> Result<Vec<Link>> {
        // Simplified implementation using scan
        let result = self.client
            .scan()
            .table_name(&self.table_name)
            .filter_expression("tenant_id = :tenant_id")
            .expression_attribute_values(":tenant_id", 
                                       AttributeValue::S(tenant_id.to_string()))
            .send()
            .await?;

        let mut links = Vec::new();
        if let Some(items) = result.items {
            for item in items {
                let link = self.item_to_link(&item).await?;
                
                // Apply filters
                if link.source.id == *source_id && link.source.entity_type == source_type {
                    if let Some(lt) = link_type {
                        if link.link_type != lt {
                            continue;
                        }
                    }
                    if let Some(tt) = target_type {
                        if link.target.entity_type != tt {
                            continue;
                        }
                    }
                    links.push(link);
                }
            }
        }
        Ok(links)
    }

    async fn find_by_target(
        &self,
        tenant_id: &Uuid,
        target_id: &Uuid,
        target_type: &str,
        link_type: Option<&str>,
        source_type: Option<&str>,
    ) -> Result<Vec<Link>> {
        // Simplified implementation using scan
        let result = self.client
            .scan()
            .table_name(&self.table_name)
            .filter_expression("tenant_id = :tenant_id")
            .expression_attribute_values(":tenant_id", 
                                       AttributeValue::S(tenant_id.to_string()))
            .send()
            .await?;

        let mut links = Vec::new();
        if let Some(items) = result.items {
            for item in items {
                let link = self.item_to_link(&item).await?;
                
                // Apply filters
                if link.target.id == *target_id && link.target.entity_type == target_type {
                    if let Some(lt) = link_type {
                        if link.link_type != lt {
                            continue;
                        }
                    }
                    if let Some(st) = source_type {
                        if link.source.entity_type != st {
                            continue;
                        }
                    }
                    links.push(link);
                }
            }
        }
        Ok(links)
    }

    async fn update(
        &self,
        tenant_id: &Uuid,
        id: &Uuid,
        metadata: Option<serde_json::Value>,
    ) -> Result<Link> {
        // Get existing link
        let mut link = self.get(tenant_id, id).await?
            .ok_or_else(|| anyhow::anyhow!("Link not found"))?;
        
        // Update metadata
        link.metadata = metadata;
        link.updated_at = chrono::Utc::now();
        
        // Save back
        let item = self.link_to_item(&link).await?;
        self.client
            .put_item()
            .table_name(&self.table_name)
            .set_item(Some(item))
            .send()
            .await?;
            
        Ok(link)
    }

    async fn delete(&self, tenant_id: &Uuid, id: &Uuid) -> Result<()> {
        let key = HashMap::from([
            ("tenant_id".to_string(), AttributeValue::S(tenant_id.to_string())),
            ("id".to_string(), AttributeValue::S(id.to_string())),
        ]);

        self.client
            .delete_item()
            .table_name(&self.table_name)
            .set_key(Some(key))
            .send()
            .await?;
            
        Ok(())
    }

    async fn delete_by_entity(
        &self,
        tenant_id: &Uuid,
        entity_id: &Uuid,
        entity_type: &str,
    ) -> Result<()> {
        // Find all links involving this entity
        let links = self.find_by_source(tenant_id, entity_id, entity_type, None, None).await?;
        let target_links = self.find_by_target(tenant_id, entity_id, entity_type, None, None).await?;
        
        // Delete all found links
        for link in links.into_iter().chain(target_links.into_iter()) {
            self.delete(tenant_id, &link.id).await?;
        }
        
        Ok(())
    }
}