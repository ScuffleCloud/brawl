use std::collections::HashSet;

use anyhow::Context;
use diesel::OptionalExtension;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use octocrab::models::pulls::ReviewState;

use super::BrawlCommandContext;
use crate::database::ci_run::{Base, CiRun};
use crate::database::pr::Pr;
use crate::github::merge_workflow::GitHubMergeWorkflow;
use crate::github::messages;
use crate::github::models::PullRequest;
use crate::github::repo::GitHubRepoClient;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeCommand {
    pub priority: Option<i32>,
}

pub async fn handle<R: GitHubRepoClient>(
    conn: &mut AsyncPgConnection,
    context: BrawlCommandContext<'_, R>,
    command: MergeCommand,
) -> anyhow::Result<()> {
    let pr = context.repo.get_pull_request(context.pr_number).await?;
    handle_with_pr(conn, pr, context, command).await
}

async fn handle_with_pr<R: GitHubRepoClient>(
    conn: &mut AsyncPgConnection,
    pr: PullRequest,
    context: BrawlCommandContext<'_, R>,
    command: MergeCommand,
) -> anyhow::Result<()> {
    if !context.repo.config().enabled {
        return Ok(());
    }

    if pr.merged_at.is_some() {
        tracing::debug!("pull request already merged");
        return Ok(());
    }

    if !context.repo.can_merge(context.user.id).await? {
        tracing::debug!("user does not have permission to do this");
        return Ok(());
    }

    if CiRun::active(context.repo.id(), pr.number)
        .get_result(conn)
        .await
        .optional()
        .context("fetch ci run")?
        .is_some()
    {
        context.repo.send_message(
            pr.number,
            &messages::error_no_body(
                "Cannot add PR to merge queue while another run is pending, use `?brawl cancel` to cancel it first & then try again."
            ),
        )
        .await?;
        return Ok(());
    }

    let mut db_pr = Pr::new(&pr, context.user.id, context.repo.id());

    if let Some(priority) = command.priority {
        db_pr.default_priority = Some(priority);
    }

    let requested_reviewers: HashSet<i64> = HashSet::from_iter(pr.requested_reviewers.iter().map(|r| r.id.0 as i64));

    let reviewers = context.repo.get_reviewers(pr.number).await?;
    let mut seen_reviewers = HashSet::new();
    let mut deduped_reviewers = Vec::new();
    for reviewer in reviewers.into_iter().rev() {
        // Skip over commented reviews since they could have approved the PR and then
        // commented. If they wanted to unapprove the PR they would have
        // dismissed their review.
        if reviewer.state == Some(ReviewState::Commented) {
            continue;
        }

        let Some(user) = &reviewer.user else {
            continue;
        };

        // Ignore the author of the PR
        if user.id.0 == db_pr.author_id as u64 {
            continue;
        }

        if seen_reviewers.insert(user.id.0 as i64) {
            deduped_reviewers.push(reviewer);
        }
    }

    let mut reviewer_ids = Vec::new();
    for reviewer in deduped_reviewers.into_iter().rev() {
        let Some(user) = reviewer.user else {
            continue;
        };

        if requested_reviewers.contains(&(user.id.0 as i64)) || reviewer.state != Some(ReviewState::Approved) {
            continue;
        }

        if context.repo.can_review(user.id).await? {
            reviewer_ids.push(user.id.0 as i64);
        }
    }

    let mut reviewer_ids = Vec::from_iter(reviewer_ids);

    reviewer_ids.sort_unstable();

    let db_pr = db_pr.upsert().get_result(conn).await?;

    // We should now start a CI Run for this PR.
    let run = CiRun::insert(context.repo.id(), pr.number)
        .base_ref(Base::from_pr(&pr))
        .head_commit_sha(pr.head.sha.as_str().into())
        .ci_branch(context.repo.config().merge_branch(&pr.base.ref_field).into())
        .maybe_priority(command.priority.or(db_pr.default_priority))
        .requested_by_id(context.user.id.0 as i64)
        .approved_by_ids(reviewer_ids)
        .is_dry_run(false)
        .build()
        .query()
        .get_result(conn)
        .await?;

    context.repo.merge_workflow().queued(&run, context.repo, conn, &db_pr).await?;

    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    use chrono::Utc;
    use octocrab::models::UserId;

    use super::*;
    use crate::command::BrawlCommand;
    use crate::database::get_test_connection;
    use crate::github::config::{GitHubBrawlRepoConfig, Permission};
    use crate::github::models::{PrBranch, PullRequest, Review, User};
    use crate::github::repo::test_utils::{MockRepoAction, MockRepoClient};

    #[derive(Default, Clone)]
    pub struct MockMergeWorkFlow {
        pub queued: Arc<AtomicBool>,
    }

    impl GitHubMergeWorkflow for MockMergeWorkFlow {
        async fn queued(
            &self,
            _: &CiRun<'_>,
            _: &impl GitHubRepoClient,
            _: &mut AsyncPgConnection,
            _: &Pr<'_>,
        ) -> anyhow::Result<()> {
            self.queued
                .compare_exchange(
                    false,
                    true,
                    std::sync::atomic::Ordering::Release,
                    std::sync::atomic::Ordering::Acquire,
                )
                .expect("failed to set queued to true");

            Ok(())
        }
    }

    #[tokio::test]
    async fn test_merge() {
        let mut conn = get_test_connection().await;

        let mock = MockMergeWorkFlow::default();

        let (client, mut rx) = MockRepoClient::new(mock.clone());

        let client = client.with_config(GitHubBrawlRepoConfig {
            reviewer_permissions: Some(vec![Permission::Team("reviewers".to_string())]),
            merge_permissions: vec![Permission::Team("mergers".to_string())],
            ..Default::default()
        });

        let task = tokio::spawn(async move {
            BrawlCommand::Merge(MergeCommand { priority: Some(100) })
                .handle(
                    &mut conn,
                    BrawlCommandContext {
                        repo: &client,
                        pr_number: 1,
                        user: User::default(),
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
                            sha: "head_sha".to_string(),
                            label: Some("head".to_string()),
                            ref_field: "head".to_string(),
                            repo: None,
                            user: None,
                        },
                        base: PrBranch {
                            sha: "base_sha".to_string(),
                            label: Some("base".to_string()),
                            ref_field: "base".to_string(),
                            repo: None,
                            user: None,
                        },
                        requested_reviewers: vec![
                            User {
                                id: UserId(1),
                                login: "test".to_string(),
                            },
                            User {
                                id: UserId(2),
                                login: "test2".to_string(),
                            },
                        ],
                        ..Default::default()
                    }))
                    .unwrap();
            }
            r => panic!("unexpected action: {:?}", r),
        }

        match rx.recv().await.unwrap() {
            MockRepoAction::HasPermission {
                user_id,
                permissions,
                result,
            } => {
                assert_eq!(user_id.0, 0);
                assert_eq!(permissions, vec![Permission::Team("mergers".to_string())]);
                result.send(Ok(true)).unwrap();
            }
            r => panic!("unexpected action: {:?}", r),
        }

        match rx.recv().await.unwrap() {
            MockRepoAction::GetReviewers { pr_number, result } => {
                assert_eq!(pr_number, 1);
                result
                    .send(Ok(vec![
                        Review {
                            state: Some(ReviewState::Approved),
                            user: Some(User {
                                id: UserId(1),
                                login: "test".to_string(),
                            }),
                        },
                        Review {
                            state: Some(ReviewState::Approved),
                            user: Some(User {
                                id: UserId(2),
                                login: "test2".to_string(),
                            }),
                        },
                        Review {
                            state: Some(ReviewState::Approved),
                            user: Some(User {
                                id: UserId(3),
                                login: "test3".to_string(),
                            }),
                        },
                        Review {
                            state: Some(ReviewState::Approved),
                            user: Some(User {
                                id: UserId(4),
                                login: "test4".to_string(),
                            }),
                        },
                        Review {
                            state: Some(ReviewState::Pending),
                            user: Some(User {
                                id: UserId(5),
                                login: "test5".to_string(),
                            }),
                        },
                    ]))
                    .unwrap();
            }
            r => panic!("unexpected action: {:?}", r),
        }

        match rx.recv().await.unwrap() {
            MockRepoAction::HasPermission {
                user_id,
                permissions,
                result,
            } => {
                assert_eq!(user_id.0, 3);
                assert_eq!(permissions, vec![Permission::Team("reviewers".to_string())]);
                result.send(Ok(true)).unwrap();
            }
            r => panic!("unexpected action: {:?}", r),
        }

        match rx.recv().await.unwrap() {
            MockRepoAction::HasPermission {
                user_id,
                permissions,
                result,
            } => {
                assert_eq!(user_id.0, 4);
                assert_eq!(permissions, vec![Permission::Team("reviewers".to_string())]);
                result.send(Ok(false)).unwrap();
            }
            r => panic!("unexpected action: {:?}", r),
        }

        let (mut conn, client) = task.await.unwrap();

        assert!(AtomicBool::load(&mock.queued, std::sync::atomic::Ordering::Acquire));

        let run = CiRun::active(client.id(), 1).get_result(&mut conn).await.unwrap();
        assert_eq!(run.requested_by_id, 0);
        assert_eq!(run.approved_by_ids, vec![3]);
        assert_eq!(run.ci_branch, "automation/brawl/merge/base");
        assert_eq!(run.base_ref, Base::Branch("base".into()));
        assert_eq!(run.head_commit_sha, "head_sha");
        assert!(!run.is_dry_run);
        assert_eq!(run.priority, 100);
    }

    #[tokio::test]
    async fn test_merge_already_running() {
        let mut conn = get_test_connection().await;

        let mock = MockMergeWorkFlow::default();

        let (client, mut rx) = MockRepoClient::new(mock.clone());

        Pr::new(&PullRequest::default(), UserId(0), client.id())
            .upsert()
            .get_result(&mut conn)
            .await
            .unwrap();

        CiRun::insert(client.id(), 0)
            .base_ref(Base::from_pr(&PullRequest::default()))
            .head_commit_sha("head_sha".into())
            .ci_branch("automation/brawl/merge/base".into())
            .requested_by_id(0)
            .approved_by_ids(vec![3])
            .is_dry_run(false)
            .priority(100)
            .build()
            .query()
            .get_result(&mut conn)
            .await
            .unwrap();

        let task = tokio::spawn(async move {
            handle_with_pr(
                &mut conn,
                PullRequest::default(),
                BrawlCommandContext {
                    repo: &client,
                    pr_number: 0,
                    user: User::default(),
                },
                MergeCommand { priority: Some(100) },
            )
            .await
            .unwrap();

            (conn, client)
        });

        match rx.recv().await.unwrap() {
            MockRepoAction::HasPermission { result, .. } => {
                result.send(Ok(true)).unwrap();
            }
            r => panic!("unexpected action: {:?}", r),
        }

        match rx.recv().await.unwrap() {
            MockRepoAction::SendMessage {
                issue_number,
                message,
                result,
            } => {
                assert_eq!(issue_number, 0);
                insta::assert_snapshot!(message, @"🚨 Cannot add PR to merge queue while another run is pending, use `?brawl cancel` to cancel it first & then try again.");
                result.send(Ok(())).unwrap();
            }
            r => panic!("unexpected action: {:?}", r),
        }

        task.await.unwrap();

        assert!(!AtomicBool::load(&mock.queued, std::sync::atomic::Ordering::Acquire));
    }

    #[tokio::test]
    async fn test_merge_already_merged() {
        let mut conn = get_test_connection().await;

        let mock = MockMergeWorkFlow::default();

        let (client, _) = MockRepoClient::new(mock.clone());

        handle_with_pr(
            &mut conn,
            PullRequest {
                merged_at: Some(Utc::now()),
                ..Default::default()
            },
            BrawlCommandContext {
                repo: &client,
                pr_number: 0,
                user: User::default(),
            },
            MergeCommand { priority: Some(100) },
        )
        .await
        .unwrap();

        assert!(!AtomicBool::load(&mock.queued, std::sync::atomic::Ordering::Acquire));
    }

    #[tokio::test]
    async fn test_merge_no_perms() {
        let mut conn = get_test_connection().await;

        let mock = MockMergeWorkFlow::default();

        let (client, mut rx) = MockRepoClient::new(mock.clone());

        let task = tokio::spawn(async move {
            handle_with_pr(
                &mut conn,
                PullRequest::default(),
                BrawlCommandContext {
                    repo: &client,
                    pr_number: 0,
                    user: User::default(),
                },
                MergeCommand { priority: Some(100) },
            )
            .await
            .unwrap();
        });

        match rx.recv().await.unwrap() {
            MockRepoAction::HasPermission { result, .. } => {
                result.send(Ok(false)).unwrap();
            }
            r => panic!("unexpected action: {:?}", r),
        }

        task.await.unwrap();

        assert!(!AtomicBool::load(&mock.queued, std::sync::atomic::Ordering::Acquire));
    }

    #[tokio::test]
    async fn test_merge_not_enabled() {
        let mut conn = get_test_connection().await;

        let (client, _) = MockRepoClient::new(MockMergeWorkFlow::default());

        let client = client.with_config(GitHubBrawlRepoConfig {
            enabled: false,
            ..Default::default()
        });

        handle_with_pr(
            &mut conn,
            PullRequest::default(),
            BrawlCommandContext {
                repo: &client,
                pr_number: 0,
                user: User::default(),
            },
            MergeCommand { priority: Some(100) },
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn test_merge_with_duplicates() {
        let mut conn = get_test_connection().await;

        let mock = MockMergeWorkFlow::default();

        let (client, mut rx) = MockRepoClient::new(mock.clone());

        let client = client.with_config(GitHubBrawlRepoConfig {
            reviewer_permissions: Some(vec![Permission::Team("reviewers".to_string())]),
            merge_permissions: vec![Permission::Team("mergers".to_string())],
            ..Default::default()
        });

        let task = tokio::spawn(async move {
            BrawlCommand::Merge(MergeCommand { priority: Some(100) })
                .handle(
                    &mut conn,
                    BrawlCommandContext {
                        repo: &client,
                        pr_number: 1,
                        user: User::default(),
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
                            sha: "head_sha".to_string(),
                            label: Some("head".to_string()),
                            ref_field: "head".to_string(),
                            repo: None,
                            user: None,
                        },
                        base: PrBranch {
                            sha: "base_sha".to_string(),
                            label: Some("base".to_string()),
                            ref_field: "base".to_string(),
                            repo: None,
                            user: None,
                        },
                        requested_reviewers: vec![],
                        ..Default::default()
                    }))
                    .unwrap();
            }
            r => panic!("unexpected action: {:?}", r),
        }

        match rx.recv().await.unwrap() {
            MockRepoAction::HasPermission {
                user_id,
                permissions,
                result,
            } => {
                assert_eq!(user_id.0, 0);
                assert_eq!(permissions, vec![Permission::Team("mergers".to_string())]);
                result.send(Ok(true)).unwrap();
            }
            r => panic!("unexpected action: {:?}", r),
        }

        match rx.recv().await.unwrap() {
            MockRepoAction::GetReviewers { pr_number, result } => {
                assert_eq!(pr_number, 1);
                result
                    .send(Ok(vec![
                        Review {
                            state: Some(ReviewState::Approved),
                            user: Some(User {
                                id: UserId(1),
                                login: "test".to_string(),
                            }),
                        },
                        Review {
                            state: Some(ReviewState::Approved),
                            user: Some(User {
                                id: UserId(1),
                                login: "test1".to_string(),
                            }),
                        },
                    ]))
                    .unwrap();
            }
            r => panic!("unexpected action: {:?}", r),
        }

        match rx.recv().await.unwrap() {
            MockRepoAction::HasPermission {
                user_id,
                permissions,
                result,
            } => {
                assert_eq!(user_id.0, 1);
                assert_eq!(permissions, vec![Permission::Team("reviewers".to_string())]);
                result.send(Ok(true)).unwrap();
            }
            r => panic!("unexpected action: {:?}", r),
        }

        let (mut conn, client) = task.await.unwrap();

        assert!(AtomicBool::load(&mock.queued, std::sync::atomic::Ordering::Acquire));

        let run = CiRun::active(client.id(), 1).get_result(&mut conn).await.unwrap();
        assert_eq!(run.requested_by_id, 0);
        assert_eq!(run.approved_by_ids, vec![1]);
        assert_eq!(run.ci_branch, "automation/brawl/merge/base");
        assert_eq!(run.base_ref, Base::Branch("base".into()));
        assert_eq!(run.head_commit_sha, "head_sha");
        assert!(!run.is_dry_run);
        assert_eq!(run.priority, 100);
    }

    #[tokio::test]
    async fn test_merge_with_commented_review() {
        let mut conn = get_test_connection().await;

        let mock = MockMergeWorkFlow::default();

        let (client, mut rx) = MockRepoClient::new(mock.clone());

        let client = client.with_config(GitHubBrawlRepoConfig {
            reviewer_permissions: Some(vec![Permission::Team("reviewers".to_string())]),
            merge_permissions: vec![Permission::Team("mergers".to_string())],
            ..Default::default()
        });

        let task = tokio::spawn(async move {
            BrawlCommand::Merge(MergeCommand { priority: Some(100) })
                .handle(
                    &mut conn,
                    BrawlCommandContext {
                        repo: &client,
                        pr_number: 1,
                        user: User::default(),
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
                            sha: "head_sha".to_string(),
                            label: Some("head".to_string()),
                            ref_field: "head".to_string(),
                            repo: None,
                            user: None,
                        },
                        base: PrBranch {
                            sha: "base_sha".to_string(),
                            label: Some("base".to_string()),
                            ref_field: "base".to_string(),
                            repo: None,
                            user: None,
                        },
                        requested_reviewers: vec![],
                        ..Default::default()
                    }))
                    .unwrap();
            }
            r => panic!("unexpected action: {:?}", r),
        }

        match rx.recv().await.unwrap() {
            MockRepoAction::HasPermission {
                user_id,
                permissions,
                result,
            } => {
                assert_eq!(user_id.0, 0);
                assert_eq!(permissions, vec![Permission::Team("mergers".to_string())]);
                result.send(Ok(true)).unwrap();
            }
            r => panic!("unexpected action: {:?}", r),
        }

        match rx.recv().await.unwrap() {
            MockRepoAction::GetReviewers { pr_number, result } => {
                assert_eq!(pr_number, 1);
                result
                    .send(Ok(vec![
                        Review {
                            state: Some(ReviewState::Approved),
                            user: Some(User {
                                id: UserId(1),
                                login: "test".to_string(),
                            }),
                        },
                        Review {
                            state: Some(ReviewState::Commented),
                            user: Some(User {
                                id: UserId(1),
                                login: "test".to_string(),
                            }),
                        },
                        Review {
                            state: Some(ReviewState::Approved),
                            user: Some(User {
                                id: UserId(0),
                                login: "test".to_string(),
                            }),
                        },
                    ]))
                    .unwrap();
            }
            r => panic!("unexpected action: {:?}", r),
        }

        match rx.recv().await.unwrap() {
            MockRepoAction::HasPermission {
                user_id,
                permissions,
                result,
            } => {
                assert_eq!(user_id.0, 1);
                assert_eq!(permissions, vec![Permission::Team("reviewers".to_string())]);
                result.send(Ok(true)).unwrap();
            }
            r => panic!("unexpected action: {:?}", r),
        }

        let (mut conn, client) = task.await.unwrap();

        assert!(AtomicBool::load(&mock.queued, std::sync::atomic::Ordering::Acquire));

        let run = CiRun::active(client.id(), 1).get_result(&mut conn).await.unwrap();
        assert_eq!(run.requested_by_id, 0);
        assert_eq!(run.approved_by_ids, vec![1]);
        assert_eq!(run.ci_branch, "automation/brawl/merge/base");
        assert_eq!(run.base_ref, Base::Branch("base".into()));
        assert_eq!(run.head_commit_sha, "head_sha");
        assert!(!run.is_dry_run);
        assert_eq!(run.priority, 100);
    }
}
