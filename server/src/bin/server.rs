#![cfg_attr(coverage_nightly, feature(coverage_attribute))]
#![cfg_attr(all(coverage_nightly, test), coverage(off))]

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use diesel::query_dsl::methods::FindDsl;
use diesel::ExpressionMethods;
use diesel_async::pooled_connection::bb8::{self};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use octocrab::models::{InstallationId, RepositoryId};
use scuffle_bootstrap_telemetry::opentelemetry;
use scuffle_bootstrap_telemetry::opentelemetry_sdk::metrics::SdkMeterProvider;
use scuffle_bootstrap_telemetry::opentelemetry_sdk::Resource;
use scuffle_bootstrap_telemetry::prometheus_client::registry::Registry;
use scuffle_brawl::database::schema::health_check;
use scuffle_brawl::database::DatabaseConnection;
use scuffle_brawl::github::models::Installation;
use scuffle_brawl::github::repo::GitHubRepoClient;
use scuffle_brawl::github::GitHubService;
use scuffle_metrics::opentelemetry::KeyValue;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer;

#[derive(Debug, smart_default::SmartDefault, serde::Deserialize)]
#[serde(default)]
pub struct Config {
    #[default = "info"]
    pub level: String,
    #[default(None)]
    pub telemetry_bind: Option<SocketAddr>,
    #[default(env_or_default("DATABASE_URL", None))]
    pub db_url: Option<String>,
    #[default(30)]
    pub interval_seconds: u64,
    pub github: GitHub,
}

#[derive(Debug, smart_default::SmartDefault, serde::Deserialize)]
#[serde(default)]
pub struct GitHub {
    #[default(SocketAddr::from(([0, 0, 0, 0], 3000)))]
    pub webhook_bind: SocketAddr,
    pub app_id: u64,
    pub private_key_pem: String,
    pub webhook_secret: String,
}

fn env_or_default<T: From<String>>(key: &'static str, default: impl Into<T>) -> T {
    std::env::var(key).map(Into::into).unwrap_or_else(|_| default.into())
}

scuffle_settings::bootstrap!(Config);

pub struct Global {
    config: Config,
    metrics_registry: Registry,
    database: bb8::Pool<AsyncPgConnection>,
    github_service: GitHubService,
}

impl scuffle_bootstrap::Global for Global {
    type Config = Config;

    async fn init(config: Self::Config) -> anyhow::Result<Arc<Self>> {
        let mut metrics_registry = Registry::default();
        let exporter = scuffle_metrics::prometheus::exporter().build();
        metrics_registry.register_collector(exporter.collector());

        opentelemetry::global::set_meter_provider(
            SdkMeterProvider::builder()
                .with_resource(Resource::new(vec![
                    KeyValue::new("service.name", env!("CARGO_PKG_NAME")),
                    KeyValue::new("service.version", env!("CARGO_PKG_VERSION")),
                ]))
                .with_reader(exporter)
                .build(),
        );

        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_file(true)
                    .with_line_number(true)
                    .with_filter(tracing_subscriber::EnvFilter::from_default_env().add_directive(config.level.parse()?)),
            )
            .init();

        tracing::info!("starting server.");

        let Some(db_url) = config.db_url.as_deref() else {
            anyhow::bail!("DATABASE_URL is not set");
        };

        tracing::info!("running migrations");

        tokio::time::timeout(std::time::Duration::from_secs(10), run_migrations(db_url))
            .await
            .context("migrations timed out")?
            .context("migrations failed")?;

        tracing::info!("migrations complete");

        let database = diesel_async::pooled_connection::bb8::Pool::builder()
            .build(diesel_async::pooled_connection::AsyncDieselConnectionManager::new(db_url))
            .await
            .context("build database pool")?;

        tracing::info!("database initialized");

        let github_service = GitHubService::new(
            config.github.app_id.into(),
            jsonwebtoken::EncodingKey::from_rsa_pem(config.github.private_key_pem.as_bytes())
                .context("decode private key")?,
        )
        .await
        .context("initialize github service")?;

        let installations = github_service.installations();
        let repo_count = installations
            .values()
            .map(|installation| installation.repositories().len())
            .sum::<usize>();

        tracing::info!(
            "github service initialized tracking {} installations with {} repositories",
            installations.len(),
            repo_count
        );

        Ok(Arc::new(Self {
            config,
            metrics_registry,
            database,
            github_service,
        }))
    }
}

async fn run_migrations(url: &str) -> anyhow::Result<()> {
    let conn = diesel_async::pg::AsyncPgConnection::establish(url)
        .await
        .context("establish connection")?;

    scuffle_brawl::migrations::run_migrations(conn)
        .await
        .context("run migrations")?;
    Ok(())
}

impl scuffle_signal::SignalConfig for Global {
    async fn on_shutdown(self: &Arc<Self>) -> anyhow::Result<()> {
        tracing::info!("shutting down server.");
        Ok(())
    }
}

impl scuffle_bootstrap_telemetry::TelemetryConfig for Global {
    async fn health_check(&self) -> Result<(), anyhow::Error> {
        let mut conn = self.database.get().await.context("get database connection")?;

        // Health check to see if the database is healthy and can be reached.
        // We do an update here because we want to make sure the database is
        // not just readable but also writable.
        diesel::update(health_check::dsl::health_check.find(1))
            .set(health_check::dsl::updated_at.eq(chrono::Utc::now()))
            .execute(&mut conn)
            .await
            .context("update health check")?;

        Ok(())
    }

    fn bind_address(&self) -> Option<std::net::SocketAddr> {
        self.config.telemetry_bind
    }

    fn prometheus_metrics_registry(&self) -> Option<&Registry> {
        Some(&self.metrics_registry)
    }
}

impl scuffle_brawl::webhook::WebhookConfig for Global {
    fn webhook_secret(&self) -> &str {
        &self.config.github.webhook_secret
    }

    fn bind_address(&self) -> Option<SocketAddr> {
        Some(self.config.github.webhook_bind)
    }

    async fn add_repo(&self, installation_id: InstallationId, repo_id: RepositoryId) -> anyhow::Result<()> {
        let client = self
            .github_service
            .get_client(installation_id)
            .context("get installation client")?;
        client.fetch_repository(repo_id).await?;
        Ok(())
    }

    async fn remove_repo(&self, installation_id: InstallationId, repo_id: RepositoryId) -> anyhow::Result<()> {
        let client = self
            .github_service
            .get_client(installation_id)
            .context("get installation client")?;
        client.remove_repository(repo_id);
        Ok(())
    }

    async fn update_installation(&self, installation: Installation) -> anyhow::Result<()> {
        self.github_service.update_installation(installation).await?;
        Ok(())
    }

    async fn delete_installation(&self, installation_id: InstallationId) -> anyhow::Result<()> {
        self.github_service.delete_installation(installation_id);
        Ok(())
    }
}

impl scuffle_brawl::auto_start::AutoStartConfig for Global {
    fn interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.config.interval_seconds)
    }
}

impl scuffle_brawl::BrawlState for Global {
    async fn get_repo(
        &self,
        installation_id: Option<InstallationId>,
        repo_id: RepositoryId,
    ) -> Option<impl GitHubRepoClient + 'static> {
        let installation = if let Some(installation_id) = installation_id {
            self.github_service.get_client(installation_id)
        } else {
            self.github_service.get_client_by_repo(repo_id)
        }?;

        installation.get_repo_client(repo_id).await
    }

    async fn database(&self) -> anyhow::Result<impl DatabaseConnection + Send> {
        let conn = self.database.get().await.context("get database connection")?;
        Ok(conn)
    }
}

scuffle_bootstrap::main! {
    Global {
        scuffle_signal::SignalSvc,
        scuffle_bootstrap_telemetry::TelemetrySvc,
        scuffle_brawl::webhook::WebhookSvc,
        scuffle_brawl::auto_start::AutoStartSvc,
    }
}
