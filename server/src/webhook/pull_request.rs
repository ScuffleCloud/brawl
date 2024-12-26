use anyhow::Context;
use diesel::OptionalExtension;
use diesel_async::{AsyncPgConnection, RunQueryDsl};

use crate::database::ci_run::{Base, CiRun};
use crate::database::enums::GithubCiRunStatus;
use crate::database::pr::Pr;
use crate::github::merge_workflow::GitHubMergeWorkflow;
use crate::github::messages;
use crate::github::models::{PullRequest, User};
use crate::github::repo::GitHubRepoClient;

pub async fn handle<R: GitHubRepoClient>(
    repo: &R,
    conn: &mut AsyncPgConnection,
    pr_number: u64,
    user: User,
) -> anyhow::Result<()> {
    let pr = repo.get_pull_request(pr_number).await?;
    handle_with_pr(repo, conn, pr, user).await
}

pub async fn handle_with_pr<R: GitHubRepoClient>(
    repo: &R,
    conn: &mut AsyncPgConnection,
    pr: PullRequest,
    user: User,
) -> anyhow::Result<()> {
    if let Some(mut current) = Pr::find(repo.id(), pr.number)
        .get_result(conn)
        .await
        .optional()
        .context("fetch pr")?
    {
        let update = current.update_from(&pr);
        if update.needs_update() {
            update.query().execute(conn).await?;
            let current_head_sha = current.latest_commit_sha.clone();
            let commit_head_changed = current_head_sha != pr.head.sha;
            update.update_pr(&mut current);

            // Fetch the active run (if there is one)
            let run = CiRun::active(repo.id(), pr.number)
                .get_result(conn)
                .await
                .optional()
                .context("fetch ci run")?;

            match &run {
                Some(run) if !run.is_dry_run => {
                    repo.merge_workflow().cancel(&run, repo, conn, &current).await?;
                    repo.send_message(
                        run.github_pr_number as u64,
                        &messages::error_no_body(format!(
                            "PR has changed while a merge was {}, cancelling the merge job.",
                            match run.status {
                                GithubCiRunStatus::Queued => "queued",
                                _ => "in progress",
                            },
                        )),
                    )
                    .await?;
                }
                Some(run) if current.auto_try && commit_head_changed && run.is_dry_run => {
                    repo.merge_workflow().cancel(&run, repo, conn, &current).await?;
                }
                _ => {}
            }

            if current.auto_try && commit_head_changed && run.is_none_or(|r| r.is_dry_run) {
                tracing::info!("starting auto-try because head changed from {} to {}", current_head_sha, pr.head.sha);
                let run = CiRun::insert(repo.id(), pr.number)
                    .base_ref(Base::from_pr(&pr))
                    .head_commit_sha(pr.head.sha.as_str().into())
                    .ci_branch(repo.config().try_branch(pr.number).into())
                    .requested_by_id(pr.head.user.as_ref().map(|u| u.id.0 as i64).unwrap_or(user.id.0 as i64))
                    .approved_by_ids(vec![])
                    .is_dry_run(true)
                    .build()
                    .query()
                    .get_result(conn)
                    .await?;

                repo.merge_workflow().start(&run, repo, conn, &current).await?;
            }
        }
    } else {
        Pr::new(&pr, user.id, repo.id())
            .insert()
            .execute(conn)
            .await
            .context("insert pr")?;
    }

    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::borrow::Cow;
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use chrono::Utc;
    use octocrab::models::UserId;

    use super::*;
    use crate::database::ci_run::Base;
    use crate::database::get_test_connection;
    use crate::github::models::{PullRequest, User};
    use crate::github::repo::test_utils::{MockRepoAction, MockRepoClient};

    #[derive(Default, Clone)]
    struct MockMergeWorkFlow {
        cancel: Arc<AtomicBool>,
    }

    impl GitHubMergeWorkflow for MockMergeWorkFlow {
        async fn cancel(
            &self,
            run: &CiRun<'_>,
            _: &impl GitHubRepoClient,
            conn: &mut AsyncPgConnection,
            _: &Pr<'_>,
        ) -> anyhow::Result<()> {
            self.cancel.store(true, std::sync::atomic::Ordering::Relaxed);

            CiRun::update(run.id)
                .status(GithubCiRunStatus::Cancelled)
                .completed_at(Utc::now())
                .build()
                .not_done()
                .execute(conn)
                .await?;

            Ok(())
        }
    }

    #[tokio::test]
    async fn test_pr_opened() {
        let mut conn = get_test_connection().await;
        let mock = MockMergeWorkFlow::default();
        let (client, mut rx) = MockRepoClient::new(mock.clone());

        let task = tokio::spawn(async move {
            handle(
                &client,
                &mut conn,
                1,
                User {
                    id: UserId(1),
                    login: "test".to_string(),
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
                        ..Default::default()
                    }))
                    .unwrap();
            }
            r => panic!("unexpected action: {:?}", r),
        }

        let (mut conn, client) = task.await.unwrap();

        assert!(!AtomicBool::load(&mock.cancel, std::sync::atomic::Ordering::Relaxed));

        let pr = Pr::find(client.id(), 1).get_result(&mut conn).await.optional().unwrap();

        assert!(pr.is_some(), "PR was not created");
    }

    #[tokio::test]
    async fn test_pr_push_while_merge() {
        let mut conn = get_test_connection().await;
        let mock = MockMergeWorkFlow::default();
        let (client, mut rx) = MockRepoClient::new(mock.clone());

        let pr = PullRequest {
            number: 1,
            ..Default::default()
        };

        Pr::new(&pr, UserId(1), client.id())
            .insert()
            .execute(&mut conn)
            .await
            .unwrap();

        CiRun::insert(client.id(), pr.number)
            .base_ref(Base::from_sha("base"))
            .head_commit_sha(Cow::Borrowed("head"))
            .ci_branch(Cow::Borrowed("ci"))
            .requested_by_id(1)
            .approved_by_ids(vec![])
            .is_dry_run(false)
            .build()
            .query()
            .get_result(&mut conn)
            .await
            .unwrap();

        let task = tokio::spawn(async move {
            handle_with_pr(
                &client,
                &mut conn,
                PullRequest {
                    number: 1,
                    title: "test".to_string(),
                    ..Default::default()
                },
                User {
                    id: UserId(1),
                    login: "test".to_string(),
                },
            )
            .await
            .unwrap();

            (conn, client)
        });

        match rx.recv().await.unwrap() {
            MockRepoAction::SendMessage {
                issue_number,
                message,
                result,
            } => {
                assert_eq!(issue_number, 1);
                insta::assert_snapshot!(message, @"🚨 PR has changed while a merge was queued, cancelling the merge job.");
                result.send(Ok(())).unwrap();
            }
            r => panic!("Expected a send message action, got {:?}", r),
        }

        let (mut conn, client) = task.await.unwrap();

        let run = CiRun::active(client.id(), 1).get_result(&mut conn).await.optional().unwrap();

        assert!(run.is_none(), "Run was not cancelled");
        assert!(AtomicBool::load(&mock.cancel, std::sync::atomic::Ordering::Relaxed));
    }

    #[tokio::test]
    async fn test_pr_push_while_merge_no_change() {
        let mut conn = get_test_connection().await;
        let mock = MockMergeWorkFlow::default();
        let (client, _) = MockRepoClient::new(mock.clone());

        let pr = PullRequest {
            number: 1,
            title: "test".to_string(),
            body: "test".to_string(),
            ..Default::default()
        };

        Pr::new(&pr, UserId(1), client.id())
            .insert()
            .execute(&mut conn)
            .await
            .unwrap();

        CiRun::insert(client.id(), pr.number)
            .base_ref(Base::from_sha("base"))
            .head_commit_sha(Cow::Borrowed("head"))
            .ci_branch(Cow::Borrowed("ci"))
            .requested_by_id(1)
            .approved_by_ids(vec![])
            .is_dry_run(false)
            .build()
            .query()
            .get_result(&mut conn)
            .await
            .unwrap();

        let task = tokio::spawn(async move {
            handle_with_pr(
                &client,
                &mut conn,
                PullRequest {
                    number: 1,
                    title: "test".to_string(),
                    body: "test".to_string(),
                    ..Default::default()
                },
                User {
                    id: UserId(1),
                    login: "test".to_string(),
                },
            )
            .await
            .unwrap();

            (conn, client)
        });

        let (mut conn, client) = task.await.unwrap();

        let run = CiRun::active(client.id(), 1).get_result(&mut conn).await.optional().unwrap();

        assert!(run.is_some(), "Run was cancelled");
        assert!(!AtomicBool::load(&mock.cancel, std::sync::atomic::Ordering::Relaxed));
    }

    #[tokio::test]
    async fn test_pr_push_while_dry_run() {
        let mut conn = get_test_connection().await;
        let mock = MockMergeWorkFlow::default();
        let (client, _) = MockRepoClient::new(mock.clone());

        let pr = PullRequest {
            number: 1,
            title: "test".to_string(),
            body: "test".to_string(),
            ..Default::default()
        };

        Pr::new(&pr, UserId(1), client.id())
            .insert()
            .execute(&mut conn)
            .await
            .unwrap();

        CiRun::insert(client.id(), pr.number)
            .base_ref(Base::from_sha("base"))
            .head_commit_sha(Cow::Borrowed("head"))
            .ci_branch(Cow::Borrowed("ci"))
            .requested_by_id(1)
            .approved_by_ids(vec![])
            .is_dry_run(true)
            .build()
            .query()
            .get_result(&mut conn)
            .await
            .unwrap();

        let task = tokio::spawn(async move {
            handle_with_pr(
                &client,
                &mut conn,
                PullRequest {
                    number: 1,
                    title: "test".to_string(),
                    body: "2".to_string(),
                    ..Default::default()
                },
                User {
                    id: UserId(1),
                    login: "test".to_string(),
                },
            )
            .await
            .unwrap();

            (conn, client)
        });

        let (mut conn, client) = task.await.unwrap();

        let run = CiRun::active(client.id(), 1).get_result(&mut conn).await.optional().unwrap();

        assert!(run.is_some(), "Run was cancelled");
        assert!(!AtomicBool::load(&mock.cancel, std::sync::atomic::Ordering::Relaxed));
    }
}
