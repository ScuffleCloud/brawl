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

pub async fn update_labels(
    conn: &mut AsyncPgConnection,
    pr: &Pr<'_>,
    status: GithubCiRunStatus,
    is_dry_run: bool,
    repo_client: &impl GitHubRepoClient,
) -> anyhow::Result<()> {
    let config = repo_client.config();
    let mut desired_labels = desired_labels(status, is_dry_run, &config.labels)
        .iter()
        .map(|s| Cow::Borrowed(s.as_ref()))
        .collect::<Vec<Cow<str>>>();

    desired_labels.sort();
    desired_labels.dedup();

    let desired_labels_set = desired_labels
        .iter()
        .map(|s| Cow::Borrowed(s.as_ref()))
        .collect::<HashSet<Cow<str>>>();

    let pr_labels_set = pr
        .added_labels
        .iter()
        .map(|l| Cow::Borrowed(l.as_ref()))
        .collect::<HashSet<Cow<str>>>();

    let labels_to_add = desired_labels_set
        .difference(&pr_labels_set)
        .map(|s| Cow::Borrowed(s.as_ref()))
        .collect::<Vec<_>>();
    let labels_to_remove = pr_labels_set
        .difference(&desired_labels_set)
        .map(|s| Cow::Borrowed(s.as_ref()))
        .collect::<Vec<_>>();

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
                "failed to add label: {:#}",
                e
            )
        }
    }

    for label in &labels_to_remove {
        if let Err(e) = repo_client.remove_label(pr.github_pr_number as u64, label.as_ref()).await {
            tracing::error!(
                repo_id = pr.github_repo_id,
                owner = repo_client.owner(),
                repo_name = repo_client.name(),
                pr_number = pr.github_pr_number,
                "failed to remove label: {:#}",
                e
            )
        }
    }

    pr.update().added_labels(desired_labels).build().query().execute(conn).await?;

    Ok(())
}
