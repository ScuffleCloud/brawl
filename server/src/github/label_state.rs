use std::borrow::Cow;

use diesel_async::{AsyncPgConnection, RunQueryDsl};

use super::config::GitHubBrawlLabelsConfig;
use super::repo::GitHubRepoClient;
use crate::database::enums::GithubCiRunStatus;
use crate::database::pr::Pr;

fn desired_labels<'a>(status: GithubCiRunStatus, is_dry_run: bool, config: &'a GitHubBrawlLabelsConfig) -> Option<&'a str> {
    match (status, is_dry_run) {
        (GithubCiRunStatus::Queued, false) => {
            if let Some(on_merge_queued) = config.on_merge_queued.as_deref() {
                return Some(on_merge_queued);
            }
        }
        (GithubCiRunStatus::InProgress, true) => {
            if let Some(on_try_in_progress) = config.on_try_in_progress.as_deref() {
                return Some(on_try_in_progress);
            }
        }
        (GithubCiRunStatus::InProgress, false) => {
            if let Some(on_merge_in_progress) = config.on_merge_in_progress.as_deref() {
                return Some(on_merge_in_progress);
            }
        }
        (GithubCiRunStatus::Failure, true) => {
            if let Some(on_try_failure) = config.on_try_failure.as_deref() {
                return Some(on_try_failure);
            }
        }
        (GithubCiRunStatus::Failure, false) => {
            if let Some(on_merge_failure) = config.on_merge_failure.as_deref() {
                return Some(on_merge_failure);
            }
        }
        (GithubCiRunStatus::Success, false) => {
            if let Some(on_merge_success) = config.on_merge_success.as_deref() {
                return Some(on_merge_success);
            }
        }
        _ => {}
    }

    None
}

pub async fn update_labels(
    conn: &mut AsyncPgConnection,
    pr: &Pr<'_>,
    status: GithubCiRunStatus,
    is_dry_run: bool,
    repo_client: &impl GitHubRepoClient,
) -> anyhow::Result<()> {
    let config = repo_client.config();
    let desired_label = desired_labels(status, is_dry_run, &config.labels);

    if desired_label == pr.added_label.as_deref() {
        return Ok(());
    }

    if let Some(desired_label) = desired_label {
        if let Err(e) = repo_client
            .add_labels(pr.github_pr_number as u64, &[desired_label.to_string()])
            .await
        {
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

    if let Some(added_label) = pr.added_label.as_deref() {
        if let Err(e) = repo_client.remove_label(pr.github_pr_number as u64, added_label).await {
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

    pr.update()
        .added_label(desired_label.map(Cow::Borrowed))
        .build()
        .query()
        .execute(conn)
        .await?;

    Ok(())
}
