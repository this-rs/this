//! Integration tests for Neo4j storage backends using the storage test harness.
//!
//! # Requirements
//!
//! - Docker must be running (testcontainers launches a Neo4j container)
//! - Feature flag `neo4j` must be enabled
//!
//! # Running
//!
//! ```sh
//! cargo test --features neo4j --test neo4j_tests -- --test-threads=1
//! ```

#![cfg(feature = "neo4j")]

#[macro_use]
mod storage_harness;

use neo4rs::Graph;
use std::sync::OnceLock;
use storage_harness::*;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::neo4j::Neo4j;
use this::storage::{Neo4jDataService, Neo4jLinkService};

// ---------------------------------------------------------------------------
// Shared test environment
// ---------------------------------------------------------------------------

struct Neo4jTestEnv {
    _container: testcontainers::ContainerAsync<testcontainers_modules::neo4j::Neo4jImage>,
    bolt_url: String,
    user: String,
    password: String,
}

static TEST_ENV: OnceLock<Neo4jTestEnv> = OnceLock::new();

async fn init_neo4j_env() -> &'static Neo4jTestEnv {
    if let Some(env) = TEST_ENV.get() {
        return env;
    }

    let neo4j = Neo4j::default();
    let container = neo4j
        .start()
        .await
        .expect("Failed to start Neo4j container — is Docker running?");

    let host = container.get_host().await.unwrap();
    let bolt_port = container.get_host_port_ipv4(7687).await.unwrap();
    let bolt_url = format!("{}:{}", host, bolt_port);

    // testcontainers-modules Neo4j sets NEO4J_AUTH=neo4j/password
    let user = "neo4j".to_string();
    let password = "password".to_string();

    // Wait for Neo4j to be ready (Bolt server may take a few seconds after port mapping).
    // Use a single auth combo to avoid triggering Neo4j 5's brute-force protection:
    // too many failed auth attempts lock out ALL connections for a cooldown period.
    let mut graph = None;
    for _attempt in 0..30 {
        let connect = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            Graph::new(&bolt_url, &user, &password),
        )
        .await;

        match connect {
            Ok(Ok(g)) => {
                let ping = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    g.run(neo4rs::query("RETURN 1")),
                )
                .await;
                if matches!(ping, Ok(Ok(_))) {
                    graph = Some(g);
                    break;
                }
            }
            _ => {}
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    let g = graph.expect("Failed to connect to Neo4j after 30 retries — check container logs");

    drop(g);

    let env = Neo4jTestEnv {
        _container: container,
        bolt_url,
        user,
        password,
    };

    let _ = TEST_ENV.set(env);
    TEST_ENV.get().unwrap()
}

async fn neo4j_graph() -> Graph {
    let env = init_neo4j_env().await;
    Graph::new(&env.bolt_url, &env.user, &env.password)
        .await
        .expect("Failed to connect to Neo4j")
}

// ---------------------------------------------------------------------------
// Factory helpers (clear data before each test)
// ---------------------------------------------------------------------------

async fn clean_neo4j_data_service() -> Neo4jDataService<TestDataEntity> {
    let graph = neo4j_graph().await;
    graph
        .run(neo4rs::query("MATCH (n:`test_data_entity`) DELETE n"))
        .await
        .expect("Failed to clean test_data_entity nodes");
    Neo4jDataService::new(graph)
}

async fn clean_neo4j_link_service() -> Neo4jLinkService {
    let graph = neo4j_graph().await;
    graph
        .run(neo4rs::query("MATCH (l:`_Link`) DELETE l"))
        .await
        .expect("Failed to clean _Link nodes");
    Neo4jLinkService::new(graph)
}

// ---------------------------------------------------------------------------
// Test suites via macros
// ---------------------------------------------------------------------------

data_service_tests!(clean_neo4j_data_service().await);
link_service_tests!(clean_neo4j_link_service().await);
rest_integration_tests!(clean_neo4j_data_service().await);
