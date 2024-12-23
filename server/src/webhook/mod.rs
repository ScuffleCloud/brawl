use std::net::SocketAddr;
use std::ops::DerefMut;
use std::sync::Arc;

use anyhow::Context;
use axum::extract::State;
use axum::http::StatusCode;
use axum::{Json, RequestExt};
use diesel_async::AsyncPgConnection;
use octocrab::models::{InstallationId, RepositoryId};
use parse::{parse_from_request, WebhookEventAction};
use scuffle_context::ContextFutExt;
use scuffle_http::backend::HttpServer;
use serde::Serialize;

mod check_event;
mod parse;
mod pull_request;

use crate::command::BrawlCommandContext;
use crate::github::models::Installation;
use crate::github::repo::GitHubRepoClient;

pub trait WebhookConfig: Send + Sync + 'static {
    fn webhook_secret(&self) -> &str;

    fn bind_address(&self) -> Option<SocketAddr>;

    fn get_repo(
        &self,
        installation_id: InstallationId,
        repo_id: RepositoryId,
    ) -> Option<Arc<impl GitHubRepoClient + 'static>>;

    fn add_repo(
        &self,
        installation_id: InstallationId,
        repo_id: RepositoryId,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;

    fn remove_repo(
        &self,
        installation_id: InstallationId,
        repo_id: RepositoryId,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;

    fn update_installation(
        &self,
        installation: Installation,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;

    fn delete_installation(&self, installation_id: InstallationId) -> anyhow::Result<()>;

    fn database(
        &self,
    ) -> impl std::future::Future<Output = anyhow::Result<impl DerefMut<Target = AsyncPgConnection> + Send>> + Send;
}

fn router<C: WebhookConfig>(global: Arc<C>) -> axum::Router {
    axum::Router::new()
        .route("/github/webhook", axum::routing::post(handle::<C>))
        .with_state(global)
}

#[derive(Debug, Serialize)]
struct Response {
    success: bool,
    message: String,
}

async fn handle<C: WebhookConfig>(
    State(global): State<Arc<C>>,
    request: axum::http::Request<axum::body::Body>,
) -> (StatusCode, Json<Response>) {
    let actions = match parse_from_request(request.with_limited_body(), global.webhook_secret()).await {
        Ok(actions) => actions,
        Err((status, message)) => {
            tracing::debug!("Failed to parse event ({}): {}", status.as_u16(), message);
            return (status, Json(Response { success: false, message }));
        }
    };

    for action in actions {
        if let Err(err) = handle_webhook_action(global.as_ref(), action).await {
            tracing::error!("Failed to handle event: {:#}", err);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(Response {
                    success: false,
                    message: "Failed to handle event".to_string(),
                }),
            );
        }
    }

    (
        StatusCode::OK,
        Json(Response {
            success: true,
            message: "Event handled successfully".to_string(),
        }),
    )
}

pub struct WebhookSvc;

impl<G> scuffle_bootstrap::Service<G> for WebhookSvc
where
    G: WebhookConfig,
{
    async fn enabled(&self, global: &Arc<G>) -> anyhow::Result<bool> {
        Ok(global.bind_address().is_some())
    }

    async fn run(self, global: Arc<G>, ctx: scuffle_context::Context) -> anyhow::Result<()> {
        let bind = global.bind_address().context("missing bind address")?;

        let server = scuffle_http::backend::tcp::TcpServerConfig::builder()
            .with_bind(bind)
            .build()
            .into_server();

        server
            .start(scuffle_http::svc::axum_service(router(global)), 1)
            .await
            .context("start")?;

        tracing::info!("webhook server started on {}", server.local_addr().context("local address")?);

        server.wait().with_context(&ctx).await.transpose().context("wait")?;

        tracing::info!("shutting down webhook server");

        server.shutdown().await.context("shutdown")?;

        tracing::info!("webhook server shutdown");

        Ok(())
    }
}

async fn handle_webhook_action(global: &impl WebhookConfig, action: WebhookEventAction) -> anyhow::Result<()> {
    match action {
        WebhookEventAction::Command {
            command,
            installation_id,
            pr_number,
            repo_id,
            user,
        } => {
            let Some(repo_client) = global.get_repo(installation_id, repo_id) else {
                return Err(anyhow::anyhow!("repo client not found"));
            };

            command
                .handle(
                    global.database().await?.deref_mut(),
                    BrawlCommandContext {
                        repo: repo_client.as_ref(),
                        user,
                        pr_number,
                    },
                )
                .await?;

            Ok(())
        }
        WebhookEventAction::PullRequest {
            installation_id,
            repo_id,
            pr_number,
            user,
        } => {
            let Some(repo_client) = global.get_repo(installation_id, repo_id) else {
                return Err(anyhow::anyhow!("repo client not found"));
            };

            pull_request::handle(repo_client.as_ref(), global.database().await?.deref_mut(), pr_number, user).await?;
            Ok(())
        }
        WebhookEventAction::CheckRun {
            check_run,
            installation_id,
            repo_id,
        } => {
            let Some(repo_client) = global.get_repo(installation_id, repo_id) else {
                return Err(anyhow::anyhow!("repo client not found"));
            };

            check_event::handle(repo_client.as_ref(), global.database().await?.deref_mut(), check_run).await?;

            Ok(())
        }
        WebhookEventAction::DeleteInstallation { installation_id } => {
            global.delete_installation(installation_id)?;
            Ok(())
        }
        WebhookEventAction::AddRepository {
            installation_id,
            repo_id,
        } => {
            global.add_repo(installation_id, repo_id).await?;
            Ok(())
        }
        WebhookEventAction::RemoveRepository {
            installation_id,
            repo_id,
        } => {
            global.remove_repo(installation_id, repo_id).await?;
            Ok(())
        }
        WebhookEventAction::UpdateInstallation { installation } => {
            global.update_installation(installation).await?;
            Ok(())
        }
    }
}
