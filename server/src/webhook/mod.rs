use std::net::SocketAddr;
use std::ops::DerefMut;
use std::str::FromStr;
use std::sync::Arc;

use anyhow::Context;
use axum::extract::State;
use axum::http::StatusCode;
use axum::{Json, RequestExt};
use diesel_async::AsyncPgConnection;
use octocrab::models::webhook_events::payload::{
    InstallationWebhookEventAction, IssueCommentWebhookEventAction, PullRequestWebhookEventAction,
};
use octocrab::models::webhook_events::WebhookEventPayload;
use octocrab::models::{InstallationId, RepositoryId};
use parse::{parse_from_request, WebhookEvent};
use scuffle_context::ContextFutExt;
use scuffle_http::backend::HttpServer;
use serde::Serialize;

mod check_event;
mod parse;

use crate::command::{BrawlCommand, BrawlCommandContext, PullRequestCommand};
use crate::github::installation::InstallationClient;
use crate::github::models::{Installation, User};
use crate::github::repo::GitHubRepoClient;

pub trait WebhookConfig: Send + Sync + 'static {
    fn webhook_secret(&self) -> &str;

    fn bind_address(&self) -> Option<SocketAddr>;

    fn installation_client(&self, installation_id: InstallationId) -> Option<Arc<InstallationClient>>;

    fn update_installation(
        &self,
        installation: Installation,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;

    fn delete_installation(&self, installation_id: InstallationId) -> anyhow::Result<()>;

    fn database(
        &self,
    ) -> impl std::future::Future<Output = anyhow::Result<impl DerefMut<Target = AsyncPgConnection> + Send>> + Send;

    fn uptime(&self) -> std::time::Duration;
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
    let event = match parse_from_request(request.with_limited_body(), global.webhook_secret()).await {
        Ok(event) => event,
        Err((status, message)) => {
            tracing::debug!("Failed to parse event ({}): {}", status.as_u16(), message);
            return (status, Json(Response { success: false, message }));
        }
    };

    if let Err(err) = handle_webhook(global.as_ref(), event).await {
        tracing::error!("Failed to handle event: {:#}", err);
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(Response {
                success: false,
                message: "Failed to handle event".to_string(),
            }),
        );
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

enum WebhookEventAction {
    Command {
        installation_id: InstallationId,
        command: BrawlCommand,
        repo_id: RepositoryId,
        pr_number: u64,
        user: User,
    },
    CheckRun {
        installation_id: InstallationId,
        repo_id: RepositoryId,
        check_run: serde_json::Value,
    },
    DeleteInstallation {
        installation_id: InstallationId,
    },
    AddRepository {
        installation_id: InstallationId,
        repo_id: RepositoryId,
    },
    RemoveRepository {
        installation_id: InstallationId,
        repo_id: RepositoryId,
    },
    UpdateInstallation {
        installation: Installation,
    },
}

async fn parse_webhook(mut event: WebhookEvent) -> anyhow::Result<Vec<WebhookEventAction>> {
    let mut actions = vec![];

    if let Some(installation) = event.installation {
        actions.push(WebhookEventAction::UpdateInstallation { installation });
    }

    let Some(installation_id) = event.installtion_id else {
        tracing::warn!("event does not have an installation id: {:?}", event.kind);
        return Ok(actions);
    };

    match event.specific {
        WebhookEventPayload::Installation(install_event) => match install_event.action {
            InstallationWebhookEventAction::Deleted | InstallationWebhookEventAction::Suspend => {
                actions.push(WebhookEventAction::DeleteInstallation { installation_id });
            }
            _ => {}
        },
        WebhookEventPayload::InstallationRepositories(event) => {
            for repo in event.repositories_added {
                actions.push(WebhookEventAction::AddRepository {
                    installation_id,
                    repo_id: repo.id,
                });
            }

            for repo in event.repositories_removed {
                actions.push(WebhookEventAction::RemoveRepository {
                    installation_id,
                    repo_id: repo.id,
                });
            }
        }
        WebhookEventPayload::Repository(_) => {
            if let Some(repo) = event.repository {
                actions.push(WebhookEventAction::AddRepository {
                    installation_id,
                    repo_id: repo.id,
                });
            }
        }
        WebhookEventPayload::PullRequest(mut pull_request_event) => {
            let command = match pull_request_event.action {
                PullRequestWebhookEventAction::Opened => BrawlCommand::PullRequest(PullRequestCommand::Opened),
                PullRequestWebhookEventAction::Synchronize => BrawlCommand::PullRequest(PullRequestCommand::Push),
                PullRequestWebhookEventAction::ConvertedToDraft => BrawlCommand::PullRequest(PullRequestCommand::IntoDraft),
                PullRequestWebhookEventAction::ReadyForReview => {
                    BrawlCommand::PullRequest(PullRequestCommand::ReadyForReview)
                }
                PullRequestWebhookEventAction::Closed => BrawlCommand::PullRequest(PullRequestCommand::Closed),
                _ => return Ok(actions),
            };

            let Some(repo_id) = event.repository.as_ref().map(|r| r.id).or(pull_request_event
                .pull_request
                .repo
                .as_deref()
                .map(|r| r.id))
            else {
                return Ok(actions);
            };

            let Some(user) = event
                .sender
                .take()
                .or_else(|| pull_request_event.pull_request.user.take().map(|u| (*u).into()))
            else {
                return Ok(actions);
            };

            actions.push(WebhookEventAction::Command {
                installation_id,
                command,
                repo_id,
                pr_number: pull_request_event.pull_request.number,
                user,
            });
        }
        WebhookEventPayload::IssueComment(issue_comment_event)
            if issue_comment_event.action == IssueCommentWebhookEventAction::Created
                && issue_comment_event.issue.pull_request.is_some() =>
        {
            let Some(body) = issue_comment_event.comment.body.as_ref() else {
                return Ok(actions);
            };

            let Ok(command) = BrawlCommand::from_str(body) else {
                return Ok(actions);
            };

            let Some(repo) = event.repository else {
                return Ok(actions);
            };

            actions.push(WebhookEventAction::Command {
                installation_id,
                command,
                repo_id: repo.id,
                pr_number: issue_comment_event.issue.number,
                user: issue_comment_event.comment.user.into(),
            });
        }
        WebhookEventPayload::CheckRun(check_run_event) => {
            let repo = event.repository.context("missing repository")?;

            actions.push(WebhookEventAction::CheckRun {
                installation_id,
                repo_id: repo.id,
                check_run: check_run_event.check_run,
            });
        }
        _ => {}
    }

    Ok(actions)
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
            let Some(installation_client) = global.installation_client(installation_id) else {
                return Err(anyhow::anyhow!("installation client not found"));
            };

            let Some(repo_client) = installation_client.get_repo_client(repo_id) else {
                return Err(anyhow::anyhow!("repo client not found"));
            };

            let pr = repo_client.get_pull_request(pr_number).await?;

            command
                .handle(
                    global.database().await?.deref_mut(),
                    BrawlCommandContext {
                        repo: repo_client.as_ref(),
                        user,
                        pr,
                    },
                )
                .await?;

            Ok(())
        }
        WebhookEventAction::CheckRun {
            check_run,
            installation_id,
            repo_id,
        } => {
            let Some(installation_client) = global.installation_client(installation_id) else {
                return Err(anyhow::anyhow!("installation client not found"));
            };

            let Some(repo_client) = installation_client.get_repo_client(repo_id) else {
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
            let Some(installation_client) = global.installation_client(installation_id) else {
                return Err(anyhow::anyhow!("installation client not found"));
            };

            installation_client.fetch_repository(repo_id).await?;
            Ok(())
        }
        WebhookEventAction::RemoveRepository {
            installation_id,
            repo_id,
        } => {
            let Some(installation_client) = global.installation_client(installation_id) else {
                return Err(anyhow::anyhow!("installation client not found"));
            };

            installation_client.remove_repository(repo_id);
            Ok(())
        }
        WebhookEventAction::UpdateInstallation { installation } => {
            global.update_installation(installation).await?;
            Ok(())
        }
    }
}

async fn handle_webhook(global: &impl WebhookConfig, event: WebhookEvent) -> anyhow::Result<()> {
    let actions = parse_webhook(event).await?;

    for action in actions {
        handle_webhook_action(global, action).await?;
    }

    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    // use super::*;
}
