use diesel_async::AsyncPgConnection;

use super::BrawlCommandContext;
use crate::github::messages;
use crate::github::repo::GitHubRepoClient;

pub async fn handle<R: GitHubRepoClient>(
    _: &mut AsyncPgConnection,
    context: BrawlCommandContext<'_, R>,
) -> anyhow::Result<()> {
    let config = context.repo.config().await?;

    // Should we also say what permissions the user has?
    context
        .repo
        .send_message(
            context.pr_number,
            &messages::pong(
                context.user.login,
                if config.is_some_and(|c| c.enabled) {
                    "enabled"
                } else {
                    "disabled"
                },
            ),
        )
        .await?;

    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use octocrab::models::UserId;

    use super::*;
    use crate::command::BrawlCommand;
    use crate::database::get_test_connection;
    use crate::github::config::GitHubBrawlRepoConfig;
    use crate::github::merge_workflow::GitHubMergeWorkflow;
    use crate::github::models::User;
    use crate::github::repo::test_utils::{MockRepoAction, MockRepoClient};

    struct MockMergeWorkFlow;

    impl GitHubMergeWorkflow for MockMergeWorkFlow {}

    #[tokio::test]
    async fn test_ping_enabled() {
        let mut conn = get_test_connection().await;
        let (client, mut rx) = MockRepoClient::new(MockMergeWorkFlow);

        tokio::spawn(async move {
            BrawlCommand::Ping
                .handle(
                    &mut conn,
                    BrawlCommandContext {
                        repo: &client,
                        pr_number: 0,
                        user: User {
                            id: UserId(1),
                            login: "troy".to_string(),
                        },
                    },
                )
                .await
                .unwrap();
        });

        match rx.recv().await.unwrap() {
            MockRepoAction::SendMessage {
                issue_number,
                message,
                result,
            } => {
                assert_eq!(issue_number, 0);
                insta::assert_snapshot!(message, @r"
                Pong, @troy!
                Brawl is currently enabled on this repo.
                ");
                result.send(Ok(())).unwrap();
            }
            _ => panic!("expected send message event"),
        }
    }

    #[tokio::test]
    async fn test_ping_disabled() {
        let mut conn = get_test_connection().await;
        let (mut client, mut rx) = MockRepoClient::new(MockMergeWorkFlow);

        client.config = Some(GitHubBrawlRepoConfig {
            enabled: false,
            ..Default::default()
        });

        tokio::spawn(async move {
            handle(
                &mut conn,
                BrawlCommandContext {
                    repo: &client,
                    pr_number: 0,
                    user: User {
                        id: UserId(1),
                        login: "troy".to_string(),
                    },
                },
            )
            .await
            .unwrap();
        });

        match rx.recv().await.unwrap() {
            MockRepoAction::SendMessage {
                issue_number,
                message,
                result,
            } => {
                assert_eq!(issue_number, 0);
                insta::assert_snapshot!(message, @r"
                Pong, @troy!
                Brawl is currently disabled on this repo.
                ");
                result.send(Ok(())).unwrap();
            }
            _ => panic!("expected send message event"),
        }
    }
}
