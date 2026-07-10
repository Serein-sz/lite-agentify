use anyhow::Context;
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection, TransactionTrait};
use tracing::info;

use crate::config::DatabaseConfig;

/// Ordered, idempotent schema migrations. Each entry runs once, tracked in
/// `schema_migrations`; new migrations append to the end and never reorder.
const MIGRATIONS: &[(&str, &str)] = &[
    (
        "0001_create_usage_records",
        r#"
        CREATE TABLE IF NOT EXISTS usage_records (
            id uuid PRIMARY KEY,
            request_id text NOT NULL,
            created_at timestamptz NOT NULL,
            provider_id text NOT NULL,
            protocol text NOT NULL,
            path text NOT NULL,
            requested_model text NULL,
            upstream_model text NULL,
            status integer NOT NULL,
            latency_ms bigint NOT NULL,
            input_tokens bigint NULL,
            output_tokens bigint NULL,
            cached_input_tokens bigint NULL,
            cache_read_tokens bigint NULL,
            cache_write_tokens bigint NULL,
            total_tokens bigint NULL,
            estimated_cost numeric(20, 10) NULL,
            currency text NULL,
            usage_source text NOT NULL,
            pricing_source text NULL
        );
        CREATE INDEX IF NOT EXISTS idx_usage_records_created_at
            ON usage_records (created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_usage_records_provider_created_at
            ON usage_records (provider_id, created_at DESC);
        CREATE INDEX IF NOT EXISTS idx_usage_records_upstream_model
            ON usage_records (upstream_model);
        "#,
    ),
    (
        "0002_create_users",
        r#"
        CREATE TABLE users (
            id uuid PRIMARY KEY,
            username text NOT NULL UNIQUE,
            password_hash text NOT NULL,
            role text NOT NULL,
            status text NOT NULL,
            created_at timestamptz NOT NULL
        );
        "#,
    ),
    (
        "0003_create_api_keys",
        r#"
        CREATE TABLE api_keys (
            id uuid PRIMARY KEY,
            user_id uuid NOT NULL REFERENCES users (id),
            key_hash text NOT NULL UNIQUE,
            prefix text NOT NULL,
            name text NOT NULL,
            status text NOT NULL,
            created_at timestamptz NOT NULL,
            last_used_at timestamptz NULL
        );
        CREATE INDEX idx_api_keys_user_id ON api_keys (user_id);
        "#,
    ),
    (
        "0004_usage_records_attribution",
        r#"
        ALTER TABLE usage_records ADD COLUMN user_id uuid NULL;
        ALTER TABLE usage_records ADD COLUMN api_key_id uuid NULL;
        CREATE INDEX idx_usage_records_user_created_at
            ON usage_records (user_id, created_at DESC);
        "#,
    ),
    (
        "0005_create_providers",
        r#"
        CREATE TABLE providers (
            id text PRIMARY KEY,
            protocol text NOT NULL,
            base_url text NOT NULL,
            api_key text NOT NULL,
            anthropic_version text NULL,
            model_aliases jsonb NOT NULL DEFAULT '{}'::jsonb,
            created_at timestamptz NOT NULL,
            updated_at timestamptz NOT NULL
        );
        "#,
    ),
    (
        "0006_create_pricing",
        r#"
        CREATE TABLE pricing (
            id uuid PRIMARY KEY,
            provider text NOT NULL,
            model text NOT NULL,
            input_per_1m numeric(20, 10) NOT NULL,
            output_per_1m numeric(20, 10) NOT NULL,
            cached_input_per_1m numeric(20, 10) NULL,
            cache_read_per_1m numeric(20, 10) NULL,
            cache_write_per_1m numeric(20, 10) NULL,
            currency text NOT NULL,
            pricing_source text NULL,
            created_at timestamptz NOT NULL,
            updated_at timestamptz NOT NULL,
            UNIQUE (provider, model)
        );
        "#,
    ),
    (
        "0007_create_model_catalog",
        r#"
        CREATE TABLE models (
            name text PRIMARY KEY,
            status text NOT NULL,
            created_at timestamptz NOT NULL,
            updated_at timestamptz NOT NULL
        );
        CREATE TABLE model_deployments (
            id uuid PRIMARY KEY,
            model_name text NOT NULL REFERENCES models (name) ON DELETE CASCADE,
            provider_id text NOT NULL REFERENCES providers (id),
            upstream_model text NOT NULL,
            priority integer NOT NULL,
            UNIQUE (model_name, provider_id)
        );
        CREATE INDEX idx_model_deployments_model ON model_deployments (model_name, priority);
        "#,
    ),
    (
        "0008_api_keys_allowed_models",
        r#"
        ALTER TABLE api_keys ADD COLUMN allowed_models jsonb NULL;
        "#,
    ),
    (
        "0009_credit_grants_and_key_caps",
        r#"
        CREATE TABLE credit_grants (
            id uuid PRIMARY KEY,
            user_id uuid NOT NULL REFERENCES users (id),
            amount_usd numeric(20, 10) NOT NULL,
            note text NULL,
            granted_by uuid NOT NULL REFERENCES users (id),
            created_at timestamptz NOT NULL
        );
        CREATE INDEX idx_credit_grants_user_created_at
            ON credit_grants (user_id, created_at DESC);
        ALTER TABLE api_keys ADD COLUMN spend_cap_usd numeric(20, 10) NULL;
        "#,
    ),
];

/// Connects the mandatory primary database. Startup fails when unreachable.
pub(crate) async fn connect(config: &DatabaseConfig) -> anyhow::Result<DatabaseConnection> {
    let mut options = ConnectOptions::new(config.url.clone());
    if let Some(max_connections) = config.max_connections {
        options.max_connections(max_connections);
    }
    Database::connect(options)
        .await
        .context("failed to connect the gateway database (the [database] section is required)")
}

/// Applies pending migrations. Each migration runs inside a transaction
/// together with its `schema_migrations` bookkeeping row.
pub(crate) async fn migrate(db: &DatabaseConnection) -> anyhow::Result<()> {
    db.execute_unprepared(
        "CREATE TABLE IF NOT EXISTS schema_migrations (
            name text PRIMARY KEY,
            applied_at timestamptz NOT NULL DEFAULT now()
        )",
    )
    .await
    .context("failed to create schema_migrations table")?;

    for (name, sql) in MIGRATIONS {
        let applied = db
            .query_one(sea_orm::Statement::from_sql_and_values(
                sea_orm::DatabaseBackend::Postgres,
                "SELECT 1 AS one FROM schema_migrations WHERE name = $1",
                [(*name).into()],
            ))
            .await
            .context("failed to query schema_migrations")?
            .is_some();
        if applied {
            continue;
        }

        let txn = db.begin().await.context("failed to begin migration")?;
        txn.execute_unprepared(sql)
            .await
            .with_context(|| format!("migration {name} failed"))?;
        txn.execute(sea_orm::Statement::from_sql_and_values(
            sea_orm::DatabaseBackend::Postgres,
            "INSERT INTO schema_migrations (name) VALUES ($1)",
            [(*name).into()],
        ))
        .await
        .with_context(|| format!("failed to record migration {name}"))?;
        txn.commit()
            .await
            .with_context(|| format!("failed to commit migration {name}"))?;
        info!(migration = name, "applied database migration");
    }

    Ok(())
}
