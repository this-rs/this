//! Contract tests for the DynamoDB storage backend, against DynamoDB Local.
//!
//! DynamoDB was the one backend of the README without ANY contract suite: its name appeared
//! in comments of `storage_harness`, and no test instantiated the harness for it. This file
//! closes that gap with the same three macros the other backends use.
//!
//! # Requirements
//!
//! - Docker must be running (testcontainers launches `amazon/dynamodb-local`)
//! - Feature flag `dynamodb` must be enabled
//!
//! # Running
//!
//! ```sh
//! cargo test --no-default-features --features dynamodb --test dynamodb_tests -- --test-threads=1
//! ```
//!
//! Each test gets its OWN table (fresh name), so tests stay independent without a shared
//! cleanup step.

#![cfg(feature = "dynamodb")]

#[macro_use]
mod storage_harness;

use aws_sdk_dynamodb::Client as DynamoClient;
use aws_sdk_dynamodb::types::{
    AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType,
};
use std::sync::OnceLock;
use storage_harness::*;
use testcontainers::core::{IntoContainerPort, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage};
use this::storage::{DynamoDBDataService, DynamoDBLinkService};

// ---------------------------------------------------------------------------
// Shared test environment: one DynamoDB Local container for the whole binary
// ---------------------------------------------------------------------------

struct DynamoTestEnv {
    _container: ContainerAsync<GenericImage>,
    endpoint: String,
}

static TEST_ENV: OnceLock<DynamoTestEnv> = OnceLock::new();

async fn init_dynamo_env() -> &'static DynamoTestEnv {
    if let Some(env) = TEST_ENV.get() {
        return env;
    }

    let container = GenericImage::new("amazon/dynamodb-local", "latest")
        .with_exposed_port(8000.tcp())
        .with_wait_for(WaitFor::message_on_stdout("Initializing DynamoDB Local"))
        .start()
        .await
        .expect("Failed to start DynamoDB Local container — is Docker running?");

    let host = container.get_host().await.expect("container host");
    let port = container
        .get_host_port_ipv4(8000.tcp())
        .await
        .expect("mapped port 8000");

    let env = DynamoTestEnv {
        _container: container,
        endpoint: format!("http://{host}:{port}"),
    };
    let _ = TEST_ENV.set(env);
    TEST_ENV.get().unwrap()
}

/// A client pointed at the local container, with static dummy credentials.
async fn dynamo_client() -> DynamoClient {
    let env = init_dynamo_env().await;
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new("us-east-1"))
        .endpoint_url(&env.endpoint)
        .credentials_provider(aws_credential_types::Credentials::new(
            "test", "test", None, None, "static",
        ))
        .load()
        .await;
    DynamoClient::new(&config)
}

/// Creates a fresh table with a single string hash key `id`. Table names are unique per call,
/// so no two tests share state.
async fn fresh_table(client: &DynamoClient) -> String {
    let name = format!("t_{}", uuid::Uuid::new_v4().simple());
    // DynamoDB Local coupe parfois une connexion quand plusieurs tables se créent à la fois
    // (« Connection reset by peer », mesuré sur le test de concurrence). C'est un défaut de
    // l'émulateur, pas du backend : on réessaie la création, au plus 5 fois, avec un délai.
    let mut derniere = None;
    for tentative in 0..5 {
        let resultat = client
            .create_table()
            .table_name(&name)
            .billing_mode(BillingMode::PayPerRequest)
            .attribute_definitions(
                AttributeDefinition::builder()
                    .attribute_name("id")
                    .attribute_type(ScalarAttributeType::S)
                    .build()
                    .expect("attribute definition"),
            )
            .key_schema(
                KeySchemaElement::builder()
                    .attribute_name("id")
                    .key_type(KeyType::Hash)
                    .build()
                    .expect("key schema"),
            )
            .send()
            .await;
        match resultat {
            Ok(_) => return name,
            Err(erreur) => {
                derniere = Some(erreur);
                tokio::time::sleep(std::time::Duration::from_millis(200 * (tentative + 1))).await;
            }
        }
    }
    panic!("create DynamoDB table after 5 attempts: {:?}", derniere);
}

async fn clean_dynamo_data_service() -> DynamoDBDataService<TestDataEntity> {
    let client = dynamo_client().await;
    let table = fresh_table(&client).await;
    DynamoDBDataService::new(client, table)
}

async fn clean_dynamo_link_service() -> DynamoDBLinkService {
    let client = dynamo_client().await;
    let table = fresh_table(&client).await;
    DynamoDBLinkService::new(client, table)
}

// ---------------------------------------------------------------------------
// Test suites via macros (the same contract every backend must pass)
// ---------------------------------------------------------------------------

data_service_tests!(clean_dynamo_data_service().await);
link_service_tests!(clean_dynamo_link_service().await);
rest_integration_tests!(clean_dynamo_data_service().await);
