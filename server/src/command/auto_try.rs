use anyhow::Context;
use diesel::OptionalExtension;
use diesel_async::{AsyncPgConnection, RunQueryDsl};

use super::BrawlCommandContext;
use crate::database::ci_run::CiRun;
use crate::database::pr::Pr;
use crate::github::messages;
use crate::github::models::PullRequest;
use crate::github::repo::GitHubRepoClient;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoTryCommand {
    Disable,
    Enable,
}

pub async fn handle<R: GitHubRepoClient>(
    conn: &mut AsyncPgConnection,
    context: BrawlCommandContext<'_, R>,
    command: AutoTryCommand,
) -> anyhow::Result<()> {
    let pr = context.repo.get_pull_request(context.pr_number).await?;
    handle_with_pr(conn, pr, context, command).await
}

async fn handle_with_pr<R: GitHubRepoClient>(
    conn: &mut AsyncPgConnection,
    pr: PullRequest,
    context: BrawlCommandContext<'_, R>,
    command: AutoTryCommand,
) -> anyhow::Result<()> {
    if !context.repo.config().enabled {
        return Ok(());
    }

    if !context.repo.can_try(context.user.id).await? {
        tracing::debug!("user does not have permission to do this");
        return Ok(());
    }

    let active_run = CiRun::active(context.repo.id(), context.pr_number)
        .get_result(conn)
        .await
        .optional()
        .context("fetch active run")?;

    if active_run.is_some_and(|r| !r.is_dry_run) {
        context.repo.send_message(context.pr_number, &messages::error_no_body("Cannot enable auto-try while a merge is in progress, use `?brawl cancel` to cancel it first & then try again.")).await?;
        return Ok(());
    }

    let db_pr = Pr::find(context.repo.id(), context.pr_number).get_result(conn).await?;

    match (command, db_pr.auto_try_requested_by_id) {
        (AutoTryCommand::Disable, Some(_)) => {
            db_pr
                .update()
                .auto_try_requested_by_id(None)
                .build()
                .query()
                .execute(conn)
                .await?;

            context
                .repo
                .send_message(context.pr_number, &messages::auto_try_disabled())
                .await?;
        }
        (AutoTryCommand::Enable, None) => {
            if pr.head.repo.is_none_or(|r| r.id != context.repo.id()) {
                context
                    .repo
                    .send_message(
                        context.pr_number,
                        &messages::error_no_body("This PR is not from this repository, so auto-try cannot be enabled."),
                    )
                    .await?;
                return Ok(());
            }

            db_pr
                .update()
                .auto_try_requested_by_id(Some(context.user.id.0 as i64))
                .build()
                .query()
                .execute(conn)
                .await?;

            context
                .repo
                .send_message(context.pr_number, &messages::auto_try_enabled(context.user.login))
                .await?;
        }
        (_, _) => {}
    }

    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::borrow::Cow;

    use octocrab::models::{RepositoryId, UserId};

    use super::*;
    use crate::command::BrawlCommand;
    use crate::database::ci_run::Base;
    use crate::github::config::{GitHubBrawlRepoConfig, Permission};
    use crate::github::merge_workflow::GitHubMergeWorkflow;
    use crate::github::models::{PrBranch, Repository, User};
    use crate::github::repo::test_utils::{MockRepoAction, MockRepoClient};

    #[derive(Debug, Clone)]
    struct MockMergeWorkflow;

    impl GitHubMergeWorkflow for MockMergeWorkflow {}

    #[tokio::test]
    async fn test_auto_try_enable() {
        let mut conn = crate::database::get_test_connection().await;

        let (client, mut rx) = MockRepoClient::new(MockMergeWorkflow);

        let client = client.with_config(GitHubBrawlRepoConfig {
            try_permissions: Some(vec![Permission::Team("try".to_string())]),
            ..Default::default()
        });

        let pr = PullRequest {
            number: 1,
            ..Default::default()
        };

        let client_id = client.id();

        Pr::new(&pr, UserId(1), client_id)
            .upsert()
            .get_result(&mut conn)
            .await
            .unwrap();

        let task = tokio::spawn(async move {
            BrawlCommand::AutoTry(AutoTryCommand::Enable)
                .handle(
                    &mut conn,
                    BrawlCommandContext {
                        repo: &client,
                        pr_number: 1,
                        user: User {
                            id: UserId(1),
                            login: "test".to_string(),
                        },
                    },
                )
                .await
                .unwrap();

            (conn, client)
        });

        match rx.recv().await.unwrap() {
            MockRepoAction::GetPullRequest { number, result } => {
                assert_eq!(number, 1);
                result
                    .send(Ok(PullRequest {
                        number: 1,
                        head: PrBranch {
                            repo: Some(Repository {
                                id: client_id,
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                        ..Default::default()
                    }))
                    .unwrap();
            }
            _ => panic!("unexpected action"),
        }

        match rx.recv().await.unwrap() {
            MockRepoAction::HasPermission {
                user_id,
                permissions,
                result,
            } => {
                assert_eq!(user_id, UserId(1));
                // First pr is a merge pr
                assert_eq!(permissions, vec![Permission::Team("try".to_string())]);
                result.send(Ok(true)).unwrap();
            }
            _ => panic!("unexpected action"),
        }

        match rx.recv().await.unwrap() {
            MockRepoAction::SendMessage {
                issue_number,
                message,
                result,
            } => {
                assert_eq!(issue_number, 1);
                insta::assert_snapshot!(message, @r"
                🟢 Auto-try enabled - Requested by @test

                Every push to this PR will automatically trigger a try run.
                ");
                result.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected action"),
        }

        let (mut conn, _) = task.await.unwrap();

        let pr = Pr::find(client_id, 1).get_result(&mut conn).await.unwrap();
        assert_eq!(pr.auto_try_requested_by_id, Some(UserId(1).0 as i64));
    }

    #[tokio::test]
    async fn test_auto_try_disable() {
        let mut conn = crate::database::get_test_connection().await;

        let (client, mut rx) = MockRepoClient::new(MockMergeWorkflow);

        let client = client.with_config(GitHubBrawlRepoConfig {
            try_permissions: Some(vec![Permission::Team("try".to_string())]),
            ..Default::default()
        });

        let pr = PullRequest {
            number: 1,
            ..Default::default()
        };

        let client_id = client.id();

        let pr = Pr::new(&pr, UserId(1), client_id)
            .upsert()
            .get_result(&mut conn)
            .await
            .unwrap();

        pr.update()
            .auto_try_requested_by_id(Some(UserId(1).0 as i64))
            .build()
            .query()
            .execute(&mut conn)
            .await
            .unwrap();

        let task = tokio::spawn(async move {
            handle_with_pr(
                &mut conn,
                PullRequest {
                    number: 1,
                    ..Default::default()
                },
                BrawlCommandContext {
                    repo: &client,
                    pr_number: 1,
                    user: User {
                        id: UserId(1),
                        login: "test".to_string(),
                    },
                },
                AutoTryCommand::Disable,
            )
            .await
            .unwrap();

            (conn, client)
        });

        match rx.recv().await.unwrap() {
            MockRepoAction::HasPermission {
                user_id,
                permissions,
                result,
            } => {
                assert_eq!(user_id, UserId(1));
                // First pr is a merge pr
                assert_eq!(permissions, vec![Permission::Team("try".to_string())]);
                result.send(Ok(true)).unwrap();
            }
            _ => panic!("unexpected action"),
        }

        match rx.recv().await.unwrap() {
            MockRepoAction::SendMessage {
                issue_number,
                message,
                result,
            } => {
                assert_eq!(issue_number, 1);
                insta::assert_snapshot!(message, @"🔴 Auto-try disabled");
                result.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected action"),
        }

        let (mut conn, _) = task.await.unwrap();

        let pr = Pr::find(client_id, 1).get_result(&mut conn).await.unwrap();
        assert_eq!(pr.auto_try_requested_by_id, None);
    }

    #[tokio::test]
    async fn test_auto_try_enable_from_fork() {
        let mut conn = crate::database::get_test_connection().await;

        let (client, mut rx) = MockRepoClient::new(MockMergeWorkflow);

        let client = client.with_config(GitHubBrawlRepoConfig {
            try_permissions: Some(vec![Permission::Team("try".to_string())]),
            ..Default::default()
        });

        let pr = PullRequest {
            number: 1,
            ..Default::default()
        };

        let client_id = client.id();

        Pr::new(&pr, UserId(1), client_id)
            .upsert()
            .get_result(&mut conn)
            .await
            .unwrap();

        let task = tokio::spawn(async move {
            handle_with_pr(
                &mut conn,
                PullRequest {
                    number: 1,
                    head: PrBranch {
                        repo: Some(Repository {
                            id: RepositoryId(1000),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    ..Default::default()
                },
                BrawlCommandContext {
                    repo: &client,
                    pr_number: 1,
                    user: User {
                        id: UserId(1),
                        login: "test".to_string(),
                    },
                },
                AutoTryCommand::Enable,
            )
            .await
            .unwrap();

            (conn, client)
        });

        match rx.recv().await.unwrap() {
            MockRepoAction::HasPermission {
                user_id,
                permissions,
                result,
            } => {
                assert_eq!(user_id, UserId(1));
                // First pr is a merge pr
                assert_eq!(permissions, vec![Permission::Team("try".to_string())]);
                result.send(Ok(true)).unwrap();
            }
            _ => panic!("unexpected action"),
        }

        match rx.recv().await.unwrap() {
            MockRepoAction::SendMessage {
                issue_number,
                message,
                result,
            } => {
                assert_eq!(issue_number, 1);
                insta::assert_snapshot!(message, @"🚨 This PR is not from this repository, so auto-try cannot be enabled.");
                result.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected action"),
        }

        tracing::info!("test_enable_from_fork: task.await.unwrap()");

        let (mut conn, _) = task.await.unwrap();

        let pr = Pr::find(client_id, 1).get_result(&mut conn).await.unwrap();
        assert_eq!(pr.auto_try_requested_by_id, None);
    }

    #[tokio::test]
    async fn test_auto_try_already_enabled() {
        let mut conn = crate::database::get_test_connection().await;

        let (client, mut rx) = MockRepoClient::new(MockMergeWorkflow);

        let client = client.with_config(GitHubBrawlRepoConfig {
            try_permissions: Some(vec![Permission::Team("try".to_string())]),
            ..Default::default()
        });

        let pr = PullRequest {
            number: 1,
            ..Default::default()
        };

        let client_id = client.id();

        Pr::new(&pr, UserId(1), client_id)
            .upsert()
            .get_result(&mut conn)
            .await
            .unwrap()
            .update()
            .auto_try_requested_by_id(Some(UserId(1).0 as i64))
            .build()
            .query()
            .execute(&mut conn)
            .await
            .unwrap();

        let task = tokio::spawn(async move {
            handle_with_pr(
                &mut conn,
                pr,
                BrawlCommandContext {
                    repo: &client,
                    pr_number: 1,
                    user: User {
                        id: UserId(100),
                        login: "test".to_string(),
                    },
                },
                AutoTryCommand::Enable,
            )
            .await
            .unwrap();

            (conn, client)
        });

        match rx.recv().await.unwrap() {
            MockRepoAction::HasPermission {
                user_id,
                permissions,
                result,
            } => {
                assert_eq!(user_id, UserId(100));
                // First pr is a merge pr
                assert_eq!(permissions, vec![Permission::Team("try".to_string())]);
                result.send(Ok(true)).unwrap();
            }
            _ => panic!("unexpected action"),
        }

        let (mut conn, _) = task.await.unwrap();

        let pr = Pr::find(client_id, 1).get_result(&mut conn).await.unwrap();
        assert_eq!(pr.auto_try_requested_by_id, Some(UserId(1).0 as i64));
    }

    #[tokio::test]
    async fn test_auto_try_no_permission() {
        let mut conn = crate::database::get_test_connection().await;

        let (client, mut rx) = MockRepoClient::new(MockMergeWorkflow);

        let client = client.with_config(GitHubBrawlRepoConfig {
            try_permissions: Some(vec![Permission::Team("try".to_string())]),
            ..Default::default()
        });

        let pr = PullRequest {
            number: 1,
            ..Default::default()
        };

        let client_id = client.id();

        Pr::new(&pr, UserId(1), client_id)
            .upsert()
            .get_result(&mut conn)
            .await
            .unwrap();

        let task = tokio::spawn(async move {
            handle_with_pr(
                &mut conn,
                pr,
                BrawlCommandContext {
                    repo: &client,
                    pr_number: 1,
                    user: User {
                        id: UserId(100),
                        login: "test".to_string(),
                    },
                },
                AutoTryCommand::Enable,
            )
            .await
            .unwrap();

            (conn, client)
        });

        match rx.recv().await.unwrap() {
            MockRepoAction::HasPermission {
                user_id,
                permissions,
                result,
            } => {
                assert_eq!(user_id, UserId(100));
                // First pr is a merge pr
                assert_eq!(permissions, vec![Permission::Team("try".to_string())]);
                result.send(Ok(false)).unwrap();
            }
            _ => panic!("unexpected action"),
        }

        let (mut conn, _) = task.await.unwrap();

        let pr = Pr::find(client_id, 1).get_result(&mut conn).await.unwrap();
        assert_eq!(pr.auto_try_requested_by_id, None);
    }

    #[tokio::test]
    async fn test_auto_try_enable_while_merge_in_progress() {
        let mut conn = crate::database::get_test_connection().await;

        let (client, mut rx) = MockRepoClient::new(MockMergeWorkflow);

        let client = client.with_config(GitHubBrawlRepoConfig {
            try_permissions: Some(vec![Permission::Team("try".to_string())]),
            ..Default::default()
        });

        let pr = PullRequest {
            number: 1,
            ..Default::default()
        };

        let client_id = client.id();

        Pr::new(&pr, UserId(1), client_id)
            .upsert()
            .get_result(&mut conn)
            .await
            .unwrap();

        CiRun::insert(client_id, 1)
            .base_ref(Base::from_sha("sha"))
            .head_commit_sha(Cow::Borrowed("sha"))
            .ci_branch(Cow::Borrowed("branch"))
            .approved_by_ids(vec![])
            .requested_by_id(UserId(1).0 as i64)
            .is_dry_run(false)
            .build()
            .query()
            .get_result(&mut conn)
            .await
            .unwrap();

        let task = tokio::spawn(async move {
            handle_with_pr(
                &mut conn,
                pr,
                BrawlCommandContext {
                    repo: &client,
                    pr_number: 1,
                    user: User {
                        id: UserId(1),
                        login: "test".to_string(),
                    },
                },
                AutoTryCommand::Enable,
            )
            .await
            .unwrap();

            (conn, client)
        });

        match rx.recv().await.unwrap() {
            MockRepoAction::HasPermission {
                user_id,
                permissions,
                result,
            } => {
                assert_eq!(user_id, UserId(1));
                assert_eq!(permissions, vec![Permission::Team("try".to_string())]);
                result.send(Ok(true)).unwrap();
            }
            _ => panic!("unexpected action"),
        }

        match rx.recv().await.unwrap() {
            MockRepoAction::SendMessage {
                issue_number,
                message,
                result,
            } => {
                assert_eq!(issue_number, 1);
                insta::assert_snapshot!(message, @"🚨 Cannot enable auto-try while a merge is in progress, use `?brawl cancel` to cancel it first & then try again.");
                result.send(Ok(())).unwrap();
            }
            _ => panic!("unexpected action"),
        }

        let (mut conn, _) = task.await.unwrap();

        let pr = Pr::find(client_id, 1).get_result(&mut conn).await.unwrap();
        assert_eq!(pr.auto_try_requested_by_id, None);
    }
}
