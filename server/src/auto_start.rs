use std::collections::HashMap;
use std::ops::DerefMut;
use std::sync::Arc;

use anyhow::Context;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use octocrab::models::RepositoryId;
use scuffle_context::ContextFutExt;

use crate::database::ci_run::CiRun;
use crate::database::pr::Pr;
use crate::github::merge_workflow::GitHubMergeWorkflow;
use crate::github::repo::GitHubRepoClient;

pub struct AutoStartSvc;

pub trait AutoStartConfig: Send + Sync + 'static {
    type RepoClient: GitHubRepoClient;

    fn repo_client(&self, repo_id: RepositoryId) -> Option<Arc<Self::RepoClient>>;

    fn database(
        &self,
    ) -> impl std::future::Future<Output = anyhow::Result<impl DerefMut<Target = AsyncPgConnection> + Send>> + Send;

    fn interval(&self) -> std::time::Duration;
}

impl<C: AutoStartConfig> scuffle_bootstrap::Service<C> for AutoStartSvc {
    async fn enabled(&self, _: &std::sync::Arc<C>) -> anyhow::Result<bool> {
        Ok(true)
    }

    async fn run(self, global: std::sync::Arc<C>, ctx: scuffle_context::Context) -> anyhow::Result<()> {
        tracing::info!("starting auto start service");

        while tokio::time::sleep(global.interval()).with_context(&ctx).await.is_some() {
            let mut conn = global.database().await?;
            let runs = get_pending_runs(&mut conn).await?;
            process_pending_runs(&mut conn, runs, global.as_ref()).await?;
        }

        Ok(())
    }
}

async fn get_pending_runs(conn: &mut AsyncPgConnection) -> anyhow::Result<Vec<CiRun<'static>>> {
    let runs = CiRun::pending().get_results(conn).await?;
    let mut run_map = HashMap::<_, CiRun>::new();
    for run in &runs {
        run_map
            .entry((run.github_repo_id, run.ci_branch.as_ref()))
            .and_modify(|current| {
                if is_higher_priority(current, run) {
                    *current = run.clone();
                }
            })
            .or_insert(run.clone());
    }

    Ok(run_map.into_values().collect())
}

fn is_higher_priority(current: &CiRun<'_>, run: &CiRun<'_>) -> bool {
    if current.started_at.is_some() {
        return false;
    }

    if run.started_at.is_some() {
        return true;
    }

    current.priority < run.priority || (current.priority == run.priority && current.id > run.id)
}

async fn process_pending_runs(
    conn: &mut AsyncPgConnection,
    runs: Vec<CiRun<'_>>,
    global: &impl AutoStartConfig,
) -> anyhow::Result<()> {
    for run in runs {
        let Some(repo_client) = global.repo_client(RepositoryId(run.github_repo_id as u64)) else {
            tracing::error!("no installation client found for repo {}", run.github_repo_id);
            continue;
        };

        if let Err(e) = handle_run(conn, &run, repo_client.as_ref()).await {
            tracing::error!(
                "error handling run (repo id: {}, pr number: {}, run id: {}): {}",
                run.github_repo_id,
                run.github_pr_number,
                run.id,
                e
            );
        }
    }

    Ok(())
}

async fn handle_run(
    conn: &mut AsyncPgConnection,
    run: &CiRun<'_>,
    repo_client: &impl GitHubRepoClient,
) -> anyhow::Result<()> {
    let pr = Pr::find(RepositoryId(run.github_repo_id as u64), run.github_pr_number as u64)
        .get_result(conn)
        .await
        .context("fetch pr")?;

    if run.started_at.is_none() {
        repo_client
            .merge_workflow()
            .start(run, repo_client, conn, &pr)
            .await
            .context("start run")?;
    } else {
        repo_client
            .merge_workflow()
            .refresh(run, repo_client, conn, &pr)
            .await
            .context("refresh run")?;
    }

    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use chrono::Utc;
    use octocrab::models::UserId;

    use super::*;
    use crate::database::ci_run::Base;
    use crate::database::enums::GithubCiRunStatus;
    use crate::database::get_test_connection;
    use crate::github::models::PullRequest;
    use crate::github::repo::test_utils::MockRepoClient;

    fn run() -> CiRun<'static> {
        CiRun {
            id: 1,
            github_repo_id: 1,
            github_pr_number: 1,
            status: GithubCiRunStatus::Queued,
            base_ref: Base::Branch("main".into()),
            head_commit_sha: "sha".into(),
            run_commit_sha: None,
            ci_branch: "branch".into(),
            approved_by_ids: vec![],
            priority: 1,
            requested_by_id: 1,
            is_dry_run: false,
            completed_at: None,
            started_at: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn test_is_higher_priority() {
        let cases = [
            (
                CiRun {
                    id: 1,
                    priority: 100,
                    ..run()
                },
                CiRun {
                    id: 2,
                    priority: 1,
                    ..run()
                },
                false,
            ),
            (
                CiRun {
                    id: 2,
                    priority: 1,
                    ..run()
                },
                CiRun {
                    id: 1,
                    priority: 1,
                    ..run()
                },
                true,
            ),
            (
                CiRun {
                    id: 1,
                    priority: 1,
                    started_at: Some(chrono::Utc::now()),
                    ..run()
                },
                CiRun {
                    id: 2,
                    priority: 100,
                    started_at: None,
                    ..run()
                },
                false,
            ),
            (
                CiRun {
                    id: 1,
                    priority: 100,
                    started_at: None,
                    ..run()
                },
                CiRun {
                    id: 2,
                    priority: 1,
                    started_at: Some(chrono::Utc::now()),
                    ..run()
                },
                true,
            ),
        ];

        for (idx, (current, run, expected)) in cases.iter().enumerate() {
            assert_eq!(
                is_higher_priority(current, run),
                *expected,
                "case {idx}: current: {:?}, run: {:?}",
                current,
                run
            );
        }
    }

    #[tokio::test]
    async fn test_get_pending_runs() {
        let mut conn = get_test_connection().await;

        Pr::new(
            &PullRequest {
                number: 1,
                ..Default::default()
            },
            UserId(1),
            RepositoryId(1),
        )
        .insert()
        .execute(&mut conn)
        .await
        .unwrap();

        Pr::new(
            &PullRequest {
                number: 2,
                ..Default::default()
            },
            UserId(1),
            RepositoryId(1),
        )
        .insert()
        .execute(&mut conn)
        .await
        .unwrap();

        Pr::new(
            &PullRequest {
                number: 3,
                ..Default::default()
            },
            UserId(1),
            RepositoryId(1),
        )
        .insert()
        .execute(&mut conn)
        .await
        .unwrap();

        Pr::new(
            &PullRequest {
                number: 4,
                ..Default::default()
            },
            UserId(1),
            RepositoryId(1),
        )
        .insert()
        .execute(&mut conn)
        .await
        .unwrap();

        Pr::new(
            &PullRequest {
                number: 1,
                ..Default::default()
            },
            UserId(1),
            RepositoryId(2),
        )
        .insert()
        .execute(&mut conn)
        .await
        .unwrap();

        Pr::new(
            &PullRequest {
                number: 5,
                ..Default::default()
            },
            UserId(1),
            RepositoryId(1),
        )
        .insert()
        .execute(&mut conn)
        .await
        .unwrap();

        CiRun::insert(RepositoryId(1), 1)
            .priority(1)
            .requested_by_id(1)
            .approved_by_ids(vec![])
            .ci_branch("main".into())
            .head_commit_sha("sha".into())
            .is_dry_run(false)
            .base_ref(Base::Branch("main".into()))
            .build()
            .query()
            .get_result(&mut conn)
            .await
            .unwrap();

        CiRun::insert(RepositoryId(1), 2)
            .priority(1)
            .requested_by_id(1)
            .approved_by_ids(vec![])
            .ci_branch("main".into())
            .head_commit_sha("sha".into())
            .is_dry_run(false)
            .base_ref(Base::Branch("main".into()))
            .build()
            .query()
            .get_result(&mut conn)
            .await
            .unwrap();

        CiRun::insert(RepositoryId(1), 3)
            .priority(1)
            .requested_by_id(1)
            .approved_by_ids(vec![])
            .ci_branch("main".into())
            .head_commit_sha("sha".into())
            .is_dry_run(false)
            .base_ref(Base::Branch("main".into()))
            .build()
            .query()
            .get_result(&mut conn)
            .await
            .unwrap();

        let run1 = CiRun::insert(RepositoryId(1), 5)
            .priority(1)
            .requested_by_id(1)
            .approved_by_ids(vec![])
            .ci_branch("main2".into())
            .head_commit_sha("sha".into())
            .is_dry_run(false)
            .base_ref(Base::Branch("main".into()))
            .build()
            .query()
            .get_result(&mut conn)
            .await
            .unwrap();

        let run2 = CiRun::insert(RepositoryId(2), 1)
            .priority(1)
            .requested_by_id(1)
            .approved_by_ids(vec![])
            .ci_branch("main".into())
            .head_commit_sha("sha".into())
            .is_dry_run(false)
            .base_ref(Base::Branch("main".into()))
            .build()
            .query()
            .get_result(&mut conn)
            .await
            .unwrap();

        let run3 = CiRun::insert(RepositoryId(1), 4)
            .priority(1)
            .requested_by_id(1)
            .approved_by_ids(vec![])
            .ci_branch("main".into())
            .head_commit_sha("sha".into())
            .is_dry_run(false)
            .base_ref(Base::Branch("main".into()))
            .build()
            .query()
            .get_result(&mut conn)
            .await
            .unwrap();

        CiRun::update(run3.id)
            .started_at(Utc::now())
            .build()
            .query()
            .execute(&mut conn)
            .await
            .unwrap();

        let runs = get_pending_runs(&mut conn).await.unwrap();
        assert_eq!(runs.len(), 3);

        assert!(runs.iter().any(|r| r.id == run1.id));
        assert!(runs.iter().any(|r| r.id == run2.id));
        assert!(runs.iter().any(|r| r.id == run3.id));

        #[derive(Default, Clone)]
        struct MockWorkflow {
            refreshed: Arc<parking_lot::Mutex<Vec<i32>>>,
            started: Arc<parking_lot::Mutex<Vec<i32>>>,
        }

        impl GitHubMergeWorkflow for MockWorkflow {
            async fn refresh(
                &self,
                run: &CiRun<'_>,
                _: &impl GitHubRepoClient,
                _: &mut AsyncPgConnection,
                _: &Pr<'_>,
            ) -> anyhow::Result<bool> {
                self.refreshed.lock().push(run.id);
                Ok(true)
            }

            async fn start(
                &self,
                run: &CiRun<'_>,
                _: &impl GitHubRepoClient,
                _: &mut AsyncPgConnection,
                _: &Pr<'_>,
            ) -> anyhow::Result<bool> {
                self.started.lock().push(run.id);
                Ok(true)
            }
        }

        let mock = MockWorkflow::default();

        let (client, _) = MockRepoClient::new(mock.clone());

        struct AutoStartCfg(Arc<MockRepoClient<MockWorkflow>>);

        impl AutoStartConfig for AutoStartCfg {
            type RepoClient = MockRepoClient<MockWorkflow>;

            fn repo_client(&self, _: RepositoryId) -> Option<Arc<Self::RepoClient>> {
                Some(self.0.clone())
            }

            fn database(
                &self,
            ) -> impl std::future::Future<Output = anyhow::Result<impl DerefMut<Target = AsyncPgConnection> + Send>> + Send
            {
                fn get_conn() -> diesel_async::pooled_connection::bb8::PooledConnection<'static, AsyncPgConnection> {
                    unreachable!()
                }

                std::future::ready(Ok(get_conn()))
            }

            fn interval(&self) -> std::time::Duration {
                todo!()
            }
        }

        let global = AutoStartCfg(Arc::new(client));

        process_pending_runs(&mut conn, runs, &global).await.unwrap();

        mock.refreshed.lock().sort();
        mock.started.lock().sort();

        assert_eq!(mock.refreshed.lock().as_slice(), &[run3.id]);
        assert_eq!(mock.started.lock().as_slice(), &[run1.id, run2.id]);
    }
}
