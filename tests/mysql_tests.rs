//! Integration tests for MySQL storage backends using the storage test harness.
//!
//! # Requirements
//!
//! - Docker must be running (testcontainers launches a MySQL container)
//! - Feature flag `mysql` must be enabled
//!
//! # Running
//!
//! ```sh
//! cargo test --features mysql --test mysql_tests -- --test-threads=1
//! ```

#![cfg(feature = "mysql")]

#[macro_use]
mod storage_harness;

use sqlx::MySqlPool;
use sqlx::mysql::MySqlPoolOptions;
use std::sync::OnceLock;
use storage_harness::*;
use testcontainers::runners::AsyncRunner;
use testcontainers_modules::mysql::Mysql;
use this::storage::mysql::ensure_schema;
use this::storage::{MysqlDataService, MysqlLinkService};

// ---------------------------------------------------------------------------
// Shared test environment
// ---------------------------------------------------------------------------

struct MysqlTestEnv {
    _container: testcontainers::ContainerAsync<Mysql>,
    connection_url: String,
}

static TEST_ENV: OnceLock<MysqlTestEnv> = OnceLock::new();

async fn init_mysql_env() -> &'static MysqlTestEnv {
    if let Some(env) = TEST_ENV.get() {
        return env;
    }

    let container = Mysql::default()
        .start()
        .await
        .expect("Failed to start MySQL container — is Docker running?");

    let host = container.get_host().await.unwrap();
    let port = container.get_host_port_ipv4(3306).await.unwrap();

    // testcontainers-modules Mysql defaults: root with no password, database "test"
    let url = format!("mysql://root@{}:{}/test", host, port);

    // MySQL needs a bit of time to become ready after port mapping
    let mut pool = None;
    for attempt in 0..60 {
        let connect =
            tokio::time::timeout(std::time::Duration::from_secs(5), MySqlPool::connect(&url)).await;

        if let Ok(Ok(p)) = connect {
            // Verify with a simple query
            let ping = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                sqlx::query("SELECT 1").execute(&p),
            )
            .await;
            if matches!(ping, Ok(Ok(_))) {
                pool = Some(p);
                break;
            }
        }

        if attempt % 10 == 0 && attempt > 0 {
            eprintln!(
                "MySQL not ready yet after {} attempts, retrying...",
                attempt
            );
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }

    let p = pool.expect("Failed to connect to MySQL after 60 retries");

    // Create schema
    ensure_schema(&p)
        .await
        .expect("Failed to create MySQL schema");

    p.close().await;

    let env = MysqlTestEnv {
        _container: container,
        connection_url: url,
    };

    let _ = TEST_ENV.set(env);
    TEST_ENV.get().unwrap()
}

async fn mysql_pool() -> MySqlPool {
    let env = init_mysql_env().await;
    MySqlPoolOptions::new()
        .max_connections(2)
        .acquire_timeout(std::time::Duration::from_secs(30))
        .connect(&env.connection_url)
        .await
        .expect("Failed to connect to MySQL")
}

// ---------------------------------------------------------------------------
// Factory helpers (truncate tables before each test for isolation)
// ---------------------------------------------------------------------------

async fn clean_mysql_data_service() -> MysqlDataService<TestDataEntity> {
    let pool = mysql_pool().await;
    sqlx::query("TRUNCATE TABLE entities")
        .execute(&pool)
        .await
        .expect("Failed to truncate entities table");
    MysqlDataService::new(pool)
}

async fn clean_mysql_link_service() -> MysqlLinkService {
    let pool = mysql_pool().await;
    sqlx::query("TRUNCATE TABLE links")
        .execute(&pool)
        .await
        .expect("Failed to truncate links table");
    MysqlLinkService::new(pool)
}

// ---------------------------------------------------------------------------
// Test suites via macros
// ---------------------------------------------------------------------------

data_service_tests!(clean_mysql_data_service().await);
link_service_tests!(clean_mysql_link_service().await);
rest_integration_tests!(clean_mysql_data_service().await);
