use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use axum::extract::State;
use axum::http::StatusCode;
use axum::{Json, RequestExt};
use diesel_async::{AsyncConnection, AsyncPgConnection};
use octocrab::models::{InstallationId, RepositoryId};
use parse::{parse_from_request, WebhookEventAction};
use scuffle_context::ContextFutExt;
use scuffle_http::backend::HttpServer;
use serde::Serialize;

mod check_event;
mod parse;
mod pull_request;

use crate::command::{BrawlCommand, BrawlCommandContext};
use crate::database::DatabaseConnection;
use crate::github::models::{Installation, User};
use crate::github::repo::GitHubRepoClient;
use crate::BrawlState;

pub trait WebhookConfig: BrawlState {
    fn webhook_secret(&self) -> &str;

    fn bind_address(&self) -> Option<SocketAddr>;

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

    fn delete_installation(
        &self,
        installation_id: InstallationId,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;

    #[doc(hidden)]
    fn handle_command<R: GitHubRepoClient + 'static>(
        &self,
        conn: &mut AsyncPgConnection,
        command: BrawlCommand,
        context: BrawlCommandContext<'_, R>,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send {
        async move { command.handle(conn, context).await }
    }

    #[doc(hidden)]
    fn handle_pull_request<R: GitHubRepoClient + 'static>(
        &self,
        conn: &mut AsyncPgConnection,
        client: &R,
        pr_number: u64,
        user: User,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send {
        async move { pull_request::handle(client, conn, pr_number, user).await }
    }

    #[doc(hidden)]
    fn handle_check_run<R: GitHubRepoClient + 'static>(
        &self,
        conn: &mut AsyncPgConnection,
        client: &R,
        check_run: serde_json::Value,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send {
        async move { check_event::handle(client, conn, check_run).await }
    }
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
            let Some(repo_client) = global.get_repo(Some(installation_id), repo_id).await else {
                return Err(anyhow::anyhow!("repo client not found"));
            };

            global
                .database()
                .await?
                .get()
                .transaction(|conn| {
                    Box::pin(async move {
                        global
                            .handle_command(
                                conn,
                                command,
                                BrawlCommandContext {
                                    repo: &repo_client,
                                    user,
                                    pr_number,
                                },
                            )
                            .await
                    })
                })
                .await
                .context("command")?;

            Ok(())
        }
        WebhookEventAction::PullRequest {
            installation_id,
            repo_id,
            pr_number,
            user,
        } => {
            let Some(repo_client) = global.get_repo(Some(installation_id), repo_id).await else {
                return Err(anyhow::anyhow!("repo client not found"));
            };

            global
                .database()
                .await?
                .get()
                .transaction(|conn| {
                    Box::pin(async move { global.handle_pull_request(conn, &repo_client, pr_number, user).await })
                })
                .await
                .context("pull request")?;

            Ok(())
        }
        WebhookEventAction::CheckRun {
            check_run,
            installation_id,
            repo_id,
        } => {
            let Some(repo_client) = global.get_repo(Some(installation_id), repo_id).await else {
                return Err(anyhow::anyhow!("repo client not found"));
            };

            global
                .database()
                .await?
                .get()
                .transaction(|conn| Box::pin(async move { global.handle_check_run(conn, &repo_client, check_run).await }))
                .await
                .context("check run")?;

            Ok(())
        }
        WebhookEventAction::DeleteInstallation { installation_id } => {
            global
                .delete_installation(installation_id)
                .await
                .context("delete installation")?;
            Ok(())
        }
        WebhookEventAction::AddRepository {
            installation_id,
            repo_id,
        } => {
            global.add_repo(installation_id, repo_id).await.context("add repository")?;
            Ok(())
        }
        WebhookEventAction::RemoveRepository {
            installation_id,
            repo_id,
        } => {
            global
                .remove_repo(installation_id, repo_id)
                .await
                .context("remove repository")?;
            Ok(())
        }
        WebhookEventAction::UpdateInstallation { installation } => {
            global
                .update_installation(installation)
                .await
                .context("update installation")?;
            Ok(())
        }
    }
}

#[cfg(test)]
#[cfg_attr(all(test, coverage_nightly), coverage(off))]
mod tests {
    use std::sync::Arc;

    use octocrab::models::UserId;
    use tokio::sync::{mpsc, oneshot};

    use super::*;
    use crate::database::get_test_connection;
    use crate::github::merge_workflow::DefaultMergeWorkflow;
    use crate::github::repo::test_utils::MockRepoClient;
    use crate::github::repo::RepoClientRef;

    #[derive(Debug)]
    enum Action {
        Command {
            command: BrawlCommand,
            pr_number: u64,
            tx: oneshot::Sender<anyhow::Result<()>>,
        },
        PullRequest {
            pr_number: u64,
            tx: oneshot::Sender<anyhow::Result<()>>,
        },
        CheckRun {
            check_run: serde_json::Value,
            tx: oneshot::Sender<anyhow::Result<()>>,
        },
        DeleteInstallation {
            installation_id: InstallationId,
            tx: oneshot::Sender<anyhow::Result<()>>,
        },
        AddRepository {
            installation_id: InstallationId,
            repo_id: RepositoryId,
            tx: oneshot::Sender<anyhow::Result<()>>,
        },
        RemoveRepository {
            installation_id: InstallationId,
            repo_id: RepositoryId,
            tx: oneshot::Sender<anyhow::Result<()>>,
        },
        UpdateInstallation {
            installation: Installation,
            tx: oneshot::Sender<anyhow::Result<()>>,
        },
        GetRepo {
            installation_id: Option<InstallationId>,
            repo_id: RepositoryId,
            tx: oneshot::Sender<Option<Arc<MockRepoClient<DefaultMergeWorkflow>>>>,
        },
    }

    struct MockWebhookConfig {
        tx: mpsc::Sender<Action>,
    }

    impl MockWebhookConfig {
        fn new() -> (Self, mpsc::Receiver<Action>) {
            let (tx, rx) = mpsc::channel(1);
            (Self { tx }, rx)
        }
    }

    impl BrawlState for MockWebhookConfig {
        async fn database(&self) -> anyhow::Result<impl DatabaseConnection + Send + '_> {
            Ok(get_test_connection().await)
        }

        async fn get_repo(
            &self,
            _installation_id: Option<InstallationId>,
            repo_id: RepositoryId,
        ) -> Option<impl GitHubRepoClient + 'static> {
            let (tx, rx) = oneshot::channel();
            self.tx
                .send(Action::GetRepo {
                    installation_id: _installation_id,
                    repo_id,
                    tx,
                })
                .await
                .expect("send get repo action");
            let repo = rx.await.expect("get repo");

            repo.map(RepoClientRef::new)
        }
    }

    impl WebhookConfig for MockWebhookConfig {
        async fn add_repo(&self, _installation_id: InstallationId, repo_id: RepositoryId) -> anyhow::Result<()> {
            let (tx, rx) = oneshot::channel();
            self.tx
                .send(Action::AddRepository {
                    installation_id: _installation_id,
                    repo_id,
                    tx,
                })
                .await
                .context("send add repository action")?;
            rx.await.context("add repository")?
        }

        async fn remove_repo(&self, _installation_id: InstallationId, repo_id: RepositoryId) -> anyhow::Result<()> {
            let (tx, rx) = oneshot::channel();
            self.tx
                .send(Action::RemoveRepository {
                    installation_id: _installation_id,
                    repo_id,
                    tx,
                })
                .await
                .context("send remove repository action")?;
            rx.await.context("remove repository")?
        }

        async fn handle_check_run<R: GitHubRepoClient + 'static>(
            &self,
            _conn: &mut AsyncPgConnection,
            _client: &R,
            check_run: serde_json::Value,
        ) -> anyhow::Result<()> {
            let (tx, rx) = oneshot::channel();
            self.tx
                .send(Action::CheckRun { check_run, tx })
                .await
                .context("send check run action")?;
            rx.await.context("check run")?
        }

        async fn handle_command<R: GitHubRepoClient + 'static>(
            &self,
            _conn: &mut AsyncPgConnection,
            command: BrawlCommand,
            context: BrawlCommandContext<'_, R>,
        ) -> anyhow::Result<()> {
            let (tx, rx) = oneshot::channel();
            self.tx
                .send(Action::Command {
                    command,
                    pr_number: context.pr_number,
                    tx,
                })
                .await
                .context("send command action")?;
            rx.await.context("command")?
        }

        async fn handle_pull_request<R: GitHubRepoClient + 'static>(
            &self,
            _conn: &mut AsyncPgConnection,
            _client: &R,
            pr_number: u64,
            _user: User,
        ) -> anyhow::Result<()> {
            let (tx, rx) = oneshot::channel();
            self.tx
                .send(Action::PullRequest { pr_number, tx })
                .await
                .context("send pull request action")?;
            rx.await.context("pull request")?
        }

        async fn update_installation(&self, installation: Installation) -> anyhow::Result<()> {
            let (tx, rx) = oneshot::channel();
            self.tx
                .send(Action::UpdateInstallation { installation, tx })
                .await
                .context("send update installation action")?;
            rx.await.context("update installation")?
        }

        async fn delete_installation(&self, installation_id: InstallationId) -> anyhow::Result<()> {
            let (tx, rx) = oneshot::channel();
            self.tx
                .send(Action::DeleteInstallation { installation_id, tx })
                .await
                .context("send delete installation action")?;
            rx.await.context("delete installation")?
        }

        fn bind_address(&self) -> Option<SocketAddr> {
            unimplemented!("bind address")
        }

        fn webhook_secret(&self) -> &str {
            unimplemented!("webhook secret")
        }
    }

    #[tokio::test]
    async fn test_handle_webhook_action_add_repository() {
        let (config, mut rx) = MockWebhookConfig::new();

        let task = tokio::spawn(async move {
            handle_webhook_action(
                &config,
                WebhookEventAction::AddRepository {
                    installation_id: InstallationId(1),
                    repo_id: RepositoryId(1),
                },
            )
            .await
            .expect("handle webhook action");
        });

        match rx.recv().await.expect("action") {
            Action::AddRepository {
                installation_id,
                repo_id,
                tx,
            } => {
                assert_eq!(installation_id, InstallationId(1));
                assert_eq!(repo_id, RepositoryId(1));
                tx.send(Ok(())).expect("send result");
            }
            r => panic!("unexpected action: {:?}", r),
        }

        task.await.expect("task");
    }

    #[tokio::test]
    async fn test_handle_webhook_action_remove_repository() {
        let (config, mut rx) = MockWebhookConfig::new();

        let task = tokio::spawn(async move {
            handle_webhook_action(
                &config,
                WebhookEventAction::RemoveRepository {
                    installation_id: InstallationId(1),
                    repo_id: RepositoryId(1),
                },
            )
            .await
            .expect("handle webhook action");
        });

        match rx.recv().await.expect("action") {
            Action::RemoveRepository {
                installation_id,
                repo_id,
                tx,
            } => {
                assert_eq!(installation_id, InstallationId(1));
                assert_eq!(repo_id, RepositoryId(1));
                tx.send(Ok(())).expect("send result");
            }
            r => panic!("unexpected action: {:?}", r),
        }

        task.await.expect("task");
    }

    #[tokio::test]
    async fn test_handle_webhook_action_delete_installation() {
        let (config, mut rx) = MockWebhookConfig::new();

        let task = tokio::spawn(async move {
            handle_webhook_action(
                &config,
                WebhookEventAction::DeleteInstallation {
                    installation_id: InstallationId(1),
                },
            )
            .await
            .expect("handle webhook action");
        });

        match rx.recv().await.expect("action") {
            Action::DeleteInstallation { installation_id, tx } => {
                assert_eq!(installation_id, InstallationId(1));
                tx.send(Ok(())).expect("send result");
            }
            r => panic!("unexpected action: {:?}", r),
        }

        task.await.expect("task");
    }

    #[tokio::test]
    async fn test_handle_webhook_action_update_installation() {
        let (config, mut rx) = MockWebhookConfig::new();

        let task = tokio::spawn(async move {
            handle_webhook_action(
                &config,
                WebhookEventAction::UpdateInstallation {
                    installation: Installation {
                        id: InstallationId(1),
                        account: User {
                            id: UserId(1),
                            login: "test".to_string(),
                        },
                    },
                },
            )
            .await
            .expect("handle webhook action");
        });

        match rx.recv().await.expect("action") {
            Action::UpdateInstallation { installation, tx } => {
                assert_eq!(installation.id, InstallationId(1));
                assert_eq!(installation.account.id, UserId(1));
                assert_eq!(installation.account.login, "test");
                tx.send(Ok(())).expect("send result");
            }
            r => panic!("unexpected action: {:?}", r),
        }

        task.await.expect("task");
    }

    #[tokio::test]
    async fn test_handle_webhook_action_check_run() {
        let (config, mut rx) = MockWebhookConfig::new();

        let task = tokio::spawn(async move {
            handle_webhook_action(
                &config,
                WebhookEventAction::CheckRun {
                    check_run: serde_json::Value::default(),
                    installation_id: InstallationId(1),
                    repo_id: RepositoryId(1),
                },
            )
            .await
            .expect("handle webhook action");
        });

        match rx.recv().await.expect("action") {
            Action::GetRepo {
                installation_id,
                repo_id,
                tx,
            } => {
                assert_eq!(installation_id, Some(InstallationId(1)));
                assert_eq!(repo_id, RepositoryId(1));
                let (client, _) = MockRepoClient::new(DefaultMergeWorkflow);
                tx.send(Some(Arc::new(client))).expect("send result");
            }
            r => panic!("unexpected action: {:?}", r),
        }

        match rx.recv().await.expect("action") {
            Action::CheckRun { check_run, tx } => {
                assert_eq!(check_run, serde_json::Value::default());
                tx.send(Ok(())).expect("send result");
            }
            r => panic!("unexpected action: {:?}", r),
        }

        task.await.expect("task");
    }

    #[tokio::test]
    async fn test_handle_webhook_action_pull_request() {
        let (config, mut rx) = MockWebhookConfig::new();

        let task = tokio::spawn(async move {
            handle_webhook_action(
                &config,
                WebhookEventAction::PullRequest {
                    installation_id: InstallationId(1),
                    repo_id: RepositoryId(1),
                    pr_number: 1,
                    user: User {
                        id: UserId(1),
                        login: "test".to_string(),
                    },
                },
            )
            .await
            .expect("handle webhook action");
        });

        match rx.recv().await.expect("action") {
            Action::GetRepo {
                installation_id,
                repo_id,
                tx,
            } => {
                assert_eq!(installation_id, Some(InstallationId(1)));
                assert_eq!(repo_id, RepositoryId(1));
                let (client, _) = MockRepoClient::new(DefaultMergeWorkflow);
                tx.send(Some(Arc::new(client))).expect("send result");
            }
            r => panic!("unexpected action: {:?}", r),
        }

        match rx.recv().await.expect("action") {
            Action::PullRequest { pr_number, tx } => {
                assert_eq!(pr_number, 1);
                tx.send(Ok(())).expect("send result");
            }
            r => panic!("unexpected action: {:?}", r),
        }

        task.await.expect("task");
    }

    #[tokio::test]
    async fn test_handle_webhook_action_command() {
        let (config, mut rx) = MockWebhookConfig::new();

        let task = tokio::spawn(async move {
            handle_webhook_action(
                &config,
                WebhookEventAction::Command {
                    installation_id: InstallationId(1),
                    command: BrawlCommand::Cancel,
                    repo_id: RepositoryId(1),
                    pr_number: 1,
                    user: User {
                        id: UserId(1),
                        login: "test".to_string(),
                    },
                },
            )
            .await
            .expect("handle webhook action");
        });

        match rx.recv().await.expect("action") {
            Action::GetRepo {
                installation_id,
                repo_id,
                tx,
            } => {
                assert_eq!(installation_id, Some(InstallationId(1)));
                assert_eq!(repo_id, RepositoryId(1));
                let (client, _) = MockRepoClient::new(DefaultMergeWorkflow);
                tx.send(Some(Arc::new(client))).expect("send result");
            }
            r => panic!("unexpected action: {:?}", r),
        }

        match rx.recv().await.expect("action") {
            Action::Command { command, pr_number, tx } => {
                assert_eq!(command, BrawlCommand::Cancel);
                assert_eq!(pr_number, 1);
                tx.send(Ok(())).expect("send result");
            }
            r => panic!("unexpected action: {:?}", r),
        }

        task.await.expect("task");
    }
}
