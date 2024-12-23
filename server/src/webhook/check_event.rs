use anyhow::Context;
use diesel::OptionalExtension;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use octocrab::models::RepositoryId;

use crate::database::ci_run::CiRun;
use crate::database::ci_run_check::CiCheck;
use crate::database::pr::Pr;
use crate::github::merge_workflow::GitHubMergeWorkflow;
use crate::github::models::CheckRunEvent;
use crate::github::repo::GitHubRepoClient;

pub async fn handle<R: GitHubRepoClient>(
    repo: &R,
    conn: &mut AsyncPgConnection,
    check_run_event: serde_json::Value,
) -> anyhow::Result<()> {
    let check_run_event: CheckRunEvent = serde_json::from_value(check_run_event).context("deserialize check run event")?;

    let Some(run) = CiRun::by_run_commit_sha(&check_run_event.head_sha)
        .get_result(conn)
        .await
        .optional()
        .context("fetch run")?
    else {
        return Ok(());
    };

    // If the run is already completed, we don't need to do anything
    if run.completed_at.is_some() {
        return Ok(());
    }

    let required = repo
        .config()
        .required_status_checks
        .iter()
        .any(|check| check.eq_ignore_ascii_case(&check_run_event.name));

    let pr = Pr::find(RepositoryId(run.github_repo_id as u64), run.github_pr_number as u64)
        .get_result(conn)
        .await
        .context("fetch pr")?;

    let ci_check = CiCheck::new(&check_run_event, run.id, required);
    ci_check.upsert().execute(conn).await.context("upsert ci check")?;

    if required {
        repo.merge_workflow().refresh(&run, repo, conn, &pr).await?;
    }

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
    use crate::database::ci_run::Base;
    use crate::database::enums::GithubCiRunStatus;
    use crate::database::get_test_connection;
    use crate::github::config::GitHubBrawlRepoConfig;
    use crate::github::models::PullRequest;
    use crate::github::repo::test_utils::MockRepoClient;

    #[derive(Clone, Default)]
    struct MockMergeWorkflow {
        refresh: Arc<AtomicBool>,
    }

    impl GitHubMergeWorkflow for MockMergeWorkflow {
        async fn refresh(
            &self,
            _: &CiRun<'_>,
            _: &impl GitHubRepoClient,
            _: &mut AsyncPgConnection,
            _: &Pr<'_>,
        ) -> anyhow::Result<bool> {
            self.refresh
                .compare_exchange(
                    false,
                    true,
                    std::sync::atomic::Ordering::Release,
                    std::sync::atomic::Ordering::Acquire,
                )
                .expect("refresh already set");
            Ok(true)
        }
    }

    #[tokio::test]
    async fn test_handle() {
        let workflow = MockMergeWorkflow::default();
        let (client, _) = MockRepoClient::new(workflow.clone());

        let mut conn = get_test_connection().await;

        Pr::new(&PullRequest::default(), UserId(1), RepositoryId(1))
            .insert()
            .execute(&mut conn)
            .await
            .unwrap();

        let run = CiRun::insert(RepositoryId(1), 0)
            .approved_by_ids(vec![1])
            .requested_by_id(1)
            .base_ref(Base::from_sha("master"))
            .head_commit_sha("dzsfdasfsad".into())
            .is_dry_run(false)
            .ci_branch("master".into())
            .build()
            .query()
            .get_result(&mut conn)
            .await
            .unwrap();

        CiRun::update(run.id)
            .run_commit_sha("c0580a621d824cc8097f88a76f47f6b313fcee99".into())
            .status(GithubCiRunStatus::InProgress)
            .build()
            .query()
            .execute(&mut conn)
            .await
            .unwrap();

        handle(
            &client,
            &mut conn,
            serde_json::from_str(include_str!("mock/check.json")).unwrap(),
        )
        .await
        .unwrap();

        // Check was not required so no refresh was called
        assert!(!AtomicBool::load(&workflow.refresh, std::sync::atomic::Ordering::Acquire));

        let client = client.with_config(GitHubBrawlRepoConfig {
            required_status_checks: vec!["audit".to_string()],
            ..Default::default()
        });

        handle(
            &client,
            &mut conn,
            serde_json::from_str(include_str!("mock/check.json")).unwrap(),
        )
        .await
        .unwrap();

        // Check was required so refresh was called
        assert!(AtomicBool::load(&workflow.refresh, std::sync::atomic::Ordering::Acquire));

        CiRun::update(run.id)
            .status(GithubCiRunStatus::Success)
            .completed_at(Utc::now())
            .build()
            .query()
            .execute(&mut conn)
            .await
            .unwrap();

        AtomicBool::store(&workflow.refresh, false, std::sync::atomic::Ordering::Release);

        handle(
            &client,
            &mut conn,
            serde_json::from_str(include_str!("mock/check.json")).unwrap(),
        )
        .await
        .unwrap();

        // Run was completed so no refresh was called
        assert!(!AtomicBool::load(&workflow.refresh, std::sync::atomic::Ordering::Acquire));
    }

    #[tokio::test]
    async fn test_handle_not_found() {
        let workflow = MockMergeWorkflow::default();
        let (client, _) = MockRepoClient::new(workflow.clone());

        let mut conn = get_test_connection().await;

        handle(
            &client,
            &mut conn,
            serde_json::from_str(include_str!("mock/check.json")).unwrap(),
        )
        .await
        .unwrap();

        // Check was not found so no refresh was called
        assert!(!AtomicBool::load(&workflow.refresh, std::sync::atomic::Ordering::Acquire));
    }
}
