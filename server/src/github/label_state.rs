use std::borrow::Cow;
use std::collections::HashSet;

use diesel_async::{AsyncPgConnection, RunQueryDsl};

use super::config::GitHubBrawlLabelsConfig;
use super::repo::GitHubRepoClient;
use crate::database::enums::GithubCiRunStatus;
use crate::database::pr::Pr;

fn desired_labels(status: GithubCiRunStatus, is_dry_run: bool, config: &GitHubBrawlLabelsConfig) -> &[String] {
    match (status, is_dry_run) {
        (GithubCiRunStatus::Queued, false) => {
            return &config.on_merge_queued;
        }
        (GithubCiRunStatus::InProgress, true) => {
            return &config.on_try_in_progress;
        }
        (GithubCiRunStatus::InProgress, false) => {
            return &config.on_merge_in_progress;
        }
        (GithubCiRunStatus::Failure, true) => {
            return &config.on_try_failure;
        }
        (GithubCiRunStatus::Failure, false) => {
            return &config.on_merge_failure;
        }
        (GithubCiRunStatus::Success, false) => {
            return &config.on_merge_success;
        }
        _ => {}
    }

    &[]
}

struct LabelsAdjustments<'a> {
    desired_labels: Vec<Cow<'a, str>>,
    labels_to_add: Vec<Cow<'a, str>>,
    labels_to_remove: Vec<Cow<'a, str>>,
}

fn get_adjustments<'a>(
    pr: &'a Pr<'a>,
    status: GithubCiRunStatus,
    is_dry_run: bool,
    config: &'a GitHubBrawlLabelsConfig,
) -> LabelsAdjustments<'a> {
    let mut desired_labels = desired_labels(status, is_dry_run, config)
        .iter()
        .map(|s| Cow::Borrowed(s.as_ref()))
        .collect::<Vec<Cow<'a, str>>>();

    desired_labels.sort();
    desired_labels.dedup();

    let desired_labels_set = desired_labels.iter().cloned().collect::<HashSet<Cow<'a, str>>>();

    let pr_labels_set = pr
        .added_labels
        .iter()
        .map(|l| Cow::Borrowed(l.as_ref()))
        .collect::<HashSet<Cow<'a, str>>>();

    let mut labels_to_add = desired_labels_set.difference(&pr_labels_set).cloned().collect::<Vec<_>>();
    let mut labels_to_remove = pr_labels_set.difference(&desired_labels_set).cloned().collect::<Vec<_>>();

    labels_to_add.sort();
    labels_to_remove.sort();

    LabelsAdjustments {
        desired_labels,
        labels_to_add,
        labels_to_remove,
    }
}

pub async fn update_labels(
    conn: &mut AsyncPgConnection,
    pr: &Pr<'_>,
    status: GithubCiRunStatus,
    is_dry_run: bool,
    repo_client: &impl GitHubRepoClient,
) -> anyhow::Result<()> {
    let config = repo_client.config();

    let LabelsAdjustments {
        desired_labels,
        labels_to_add,
        labels_to_remove,
    } = get_adjustments(pr, status, is_dry_run, &config.labels);

    if labels_to_add.is_empty() && labels_to_remove.is_empty() {
        return Ok(());
    }

    // I am not sure if we need to make an API call for each label
    // The reason I do this is because each call can fail if the label does not
    // exist or already has been added In which case I don't want to fail the
    // entire update
    for label in &labels_to_add {
        if let Err(e) = repo_client.add_labels(pr.github_pr_number as u64, &[label.to_string()]).await {
            tracing::error!(
                repo_id = pr.github_repo_id,
                owner = repo_client.owner(),
                repo_name = repo_client.name(),
                pr_number = pr.github_pr_number,
                label = %label,
                "failed to add label: {:#}",
                e
            );
        }
    }

    for label in &labels_to_remove {
        if let Err(e) = repo_client.remove_label(pr.github_pr_number as u64, label.as_ref()).await {
            tracing::error!(
                repo_id = pr.github_repo_id,
                owner = repo_client.owner(),
                repo_name = repo_client.name(),
                pr_number = pr.github_pr_number,
                label = %label,
                "failed to remove label: {:#}",
                e
            );
        }
    }

    pr.update().added_labels(desired_labels).build().query().execute(conn).await?;

    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use octocrab::models::{RepositoryId, UserId};

    use super::*;
    use crate::database::get_test_connection;
    use crate::github::merge_workflow::DefaultMergeWorkflow;
    use crate::github::models::{Label, PullRequest};
    use crate::github::repo::test_utils::{MockRepoAction, MockRepoClient};

    #[test]
    fn test_desired_labels() {
        let config = GitHubBrawlLabelsConfig {
            on_merge_queued: vec!["queued".to_string()],
            on_try_in_progress: vec!["try".to_string(), "in_progress".to_string()],
            on_merge_in_progress: vec!["merge".to_string(), "in_progress".to_string()],
            on_try_failure: vec!["try".to_string(), "failure".to_string()],
            on_merge_failure: vec!["merge".to_string(), "failure".to_string()],
            on_merge_success: vec!["merge".to_string(), "success".to_string()],
        };

        let cases: &[(GithubCiRunStatus, bool, &[&str])] = &[
            (GithubCiRunStatus::Queued, false, &["queued"]),
            (GithubCiRunStatus::InProgress, false, &["merge", "in_progress"]),
            (GithubCiRunStatus::InProgress, true, &["try", "in_progress"]),
            (GithubCiRunStatus::Failure, false, &["merge", "failure"]),
            (GithubCiRunStatus::Failure, true, &["try", "failure"]),
            (GithubCiRunStatus::Success, false, &["merge", "success"]),
            (GithubCiRunStatus::Cancelled, false, &[]),
            (GithubCiRunStatus::Cancelled, true, &[]),
        ];

        for (status, is_dry_run, expected) in cases {
            assert_eq!(desired_labels(*status, *is_dry_run, &config), *expected);
        }
    }

    #[test]
    fn test_labels_adjustments() {
        let pr = PullRequest::default();
        let mut pr = Pr::new(&pr, UserId(1), RepositoryId(1));

        let config = GitHubBrawlLabelsConfig {
            on_merge_queued: vec!["queued".to_string()],
            on_try_in_progress: vec!["try".to_string(), "in_progress".to_string()],
            on_merge_in_progress: vec!["merge".to_string(), "in_progress".to_string()],
            on_try_failure: vec!["try".to_string(), "failure".to_string()],
            on_merge_failure: vec!["merge".to_string(), "failure".to_string()],
            on_merge_success: vec!["merge".to_string(), "success".to_string()],
        };

        let LabelsAdjustments {
            desired_labels,
            labels_to_add,
            labels_to_remove,
        } = get_adjustments(&pr, GithubCiRunStatus::Queued, false, &config);
        assert_eq!(desired_labels, &["queued"]);
        assert_eq!(labels_to_add, &["queued"]);
        assert!(labels_to_remove.is_empty());

        pr.added_labels = vec![Cow::Borrowed("queued")];
        let LabelsAdjustments {
            desired_labels,
            labels_to_add,
            labels_to_remove,
        } = get_adjustments(&pr, GithubCiRunStatus::Queued, false, &config);
        assert_eq!(desired_labels, &["queued"]);
        assert!(labels_to_add.is_empty());
        assert!(labels_to_remove.is_empty());

        pr.added_labels = vec![Cow::Borrowed("queued"), Cow::Borrowed("merge")];
        let LabelsAdjustments {
            desired_labels,
            labels_to_add,
            labels_to_remove,
        } = get_adjustments(&pr, GithubCiRunStatus::Queued, false, &config);
        assert_eq!(desired_labels, &["queued"]);
        assert!(labels_to_add.is_empty());
        assert_eq!(labels_to_remove, &["merge"]);

        pr.added_labels = vec![Cow::Borrowed("queued"), Cow::Borrowed("merge")];
        let LabelsAdjustments {
            desired_labels,
            labels_to_add,
            labels_to_remove,
        } = get_adjustments(&pr, GithubCiRunStatus::InProgress, false, &config);
        assert_eq!(desired_labels, &["in_progress", "merge"]);
        assert_eq!(labels_to_add, &["in_progress"]);
        assert_eq!(labels_to_remove, &["queued"]);
    }

    #[tokio::test]
    async fn test_update_labels() {
        let mut conn = get_test_connection().await;
        let pr = PullRequest::default();
        let pr = Pr::new(&pr, UserId(1), RepositoryId(1));

        pr.insert().execute(&mut conn).await.unwrap();

        let pr = Pr::find(RepositoryId(1), pr.github_pr_number as u64)
            .get_result(&mut conn)
            .await
            .unwrap();

        let (mut repo_client, mut rx) = MockRepoClient::new(DefaultMergeWorkflow);

        repo_client.config.labels = GitHubBrawlLabelsConfig {
            on_merge_queued: vec!["queued".to_string()],
            on_try_in_progress: vec!["try".to_string(), "in_progress".to_string()],
            on_merge_in_progress: vec!["merge".to_string(), "in_progress".to_string()],
            on_try_failure: vec!["try".to_string(), "failure".to_string()],
            on_merge_failure: vec!["merge".to_string(), "failure".to_string()],
            on_merge_success: vec!["merge".to_string(), "success".to_string()],
        };

        let pr_number = pr.github_pr_number as u64;

        let task = tokio::spawn(async move {
            update_labels(&mut conn, &pr, GithubCiRunStatus::Queued, false, &repo_client)
                .await
                .unwrap();
            (conn, repo_client)
        });

        match rx.recv().await.unwrap() {
            MockRepoAction::AddLabels {
                issue_number,
                labels,
                result,
            } => {
                assert_eq!(issue_number, pr_number);
                assert_eq!(labels, vec!["queued".to_string()]);
                result
                    .send(Ok(vec![Label {
                        name: "queued".to_string(),
                    }]))
                    .expect("failed to send result");
            }
            _ => panic!("expected AddLabels event"),
        }

        let (mut conn, repo_client) = task.await.unwrap();

        let pr = Pr::find(RepositoryId(1), pr_number).get_result(&mut conn).await.unwrap();
        assert_eq!(pr.added_labels, vec!["queued"]);

        let task = tokio::spawn(async move {
            update_labels(&mut conn, &pr, GithubCiRunStatus::InProgress, false, &repo_client)
                .await
                .unwrap();
            (conn, repo_client)
        });

        match rx.recv().await.unwrap() {
            MockRepoAction::AddLabels {
                issue_number,
                labels,
                result,
            } => {
                assert_eq!(issue_number, pr_number);
                assert_eq!(labels, vec!["in_progress".to_string()]);
                result
                    .send(Ok(vec![
                        Label {
                            name: "in_progress".to_string(),
                        },
                        Label {
                            name: "queued".to_string(),
                        },
                    ]))
                    .expect("failed to send result");
            }
            _ => panic!("expected AddLabels event"),
        }

        match rx.recv().await.unwrap() {
            MockRepoAction::AddLabels {
                issue_number,
                labels,
                result,
            } => {
                assert_eq!(issue_number, pr_number);
                assert_eq!(labels, vec!["merge".to_string()]);
                result
                    .send(Ok(vec![
                        Label {
                            name: "merge".to_string(),
                        },
                        Label {
                            name: "in_progress".to_string(),
                        },
                        Label {
                            name: "queued".to_string(),
                        },
                    ]))
                    .expect("failed to send result");
            }
            _ => panic!("expected AddLabels event"),
        }

        match rx.recv().await.unwrap() {
            MockRepoAction::RemoveLabel {
                issue_number,
                label,
                result,
            } => {
                assert_eq!(issue_number, pr_number);
                assert_eq!(label, "queued");
                result
                    .send(Ok(vec![
                        Label {
                            name: "merge".to_string(),
                        },
                        Label {
                            name: "in_progress".to_string(),
                        },
                    ]))
                    .expect("failed to send result");
            }
            _ => panic!("expected RemoveLabel event"),
        }

        let (mut conn, _) = task.await.unwrap();

        let pr = Pr::find(RepositoryId(1), pr_number).get_result(&mut conn).await.unwrap();
        assert_eq!(pr.added_labels, vec!["in_progress", "merge"]);
    }
}
