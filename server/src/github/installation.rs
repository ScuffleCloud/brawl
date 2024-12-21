use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::http;
use futures::TryStreamExt;
use moka::future::Cache;
use octocrab::models::{Installation, InstallationRepositories, RepositoryId, UserId};
use octocrab::{GitHubError, Octocrab};
use parking_lot::Mutex;

use super::config::GitHubBrawlRepoConfig;
use super::merge_workflow::DefaultMergeWorkflow;
use super::models::{Repository, User};
use super::repo::RepoClient;

pub struct InstallationClient {
    client: Octocrab,
    installation: Mutex<Installation>,
    repositories: Mutex<HashMap<RepositoryId, Arc<RepoClient>>>,
    user_cache: UserCache,
}

#[derive(Debug, thiserror::Error)]
pub enum GitHubBrawlRepoConfigError {
    #[error("expected 1 file, got {0}")]
    ExpectedOneFile(usize),
    #[error("github error: {0}")]
    GitHub(#[from] octocrab::Error),
    #[error("toml error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("missing content")]
    MissingContent,
}

#[derive(Debug, Clone)]
pub struct UserCache {
    users: Cache<UserId, Option<User>>,
    users_by_name: Cache<String, Option<UserId>>,
    teams: Cache<String, Vec<UserId>>,
}

impl Default for UserCache {
    fn default() -> Self {
        Self::new(Duration::from_secs(60), 100)
    }
}

impl UserCache {
    pub fn new(ttl: Duration, capacity: u64) -> Self {
        Self {
            users: Cache::builder().max_capacity(capacity).time_to_live(ttl).build(),
            users_by_name: Cache::builder().max_capacity(capacity).time_to_live(ttl).build(),
            teams: Cache::builder().max_capacity(capacity).time_to_live(ttl).build(),
        }
    }
}

impl InstallationClient {
    pub async fn new(client: Octocrab, installation: Installation) -> anyhow::Result<Self> {
        Ok(Self {
            client,
            installation: Mutex::new(installation),
            repositories: Mutex::new(HashMap::new()),
            user_cache: UserCache::default(),
        })
    }

    async fn get_repo_config(&self, repo_id: RepositoryId) -> Result<GitHubBrawlRepoConfig, GitHubBrawlRepoConfigError> {
        let file = match self
            .client
            .repos_by_id(repo_id)
            .get_content()
            .path(".github/brawl.toml")
            .send()
            .await
        {
            Ok(file) => file,
            Err(octocrab::Error::GitHub {
                source:
                    GitHubError {
                        status_code: http::StatusCode::NOT_FOUND,
                        ..
                    },
                ..
            }) => {
                return Ok(GitHubBrawlRepoConfig::missing());
            }
            Err(e) => return Err(e.into()),
        };

        if file.items.is_empty() {
            return Ok(GitHubBrawlRepoConfig::missing());
        }

        if file.items.len() != 1 {
            return Err(GitHubBrawlRepoConfigError::ExpectedOneFile(file.items.len()));
        }

        let config = toml::from_str(
            &file.items[0]
                .decoded_content()
                .ok_or(GitHubBrawlRepoConfigError::MissingContent)?,
        )?;

        Ok(config)
    }

    async fn set_repository(self: &Arc<Self>, repo: Repository) -> anyhow::Result<()> {
        let config = self.get_repo_config(repo.id).await.context("get repo config")?;
        match self.repositories.lock().entry(repo.id) {
            Entry::Occupied(entry) => {
                entry.get().config.store(Arc::new(config));
                entry.get().repo.store(Arc::new(repo));
            }
            Entry::Vacant(entry) => {
                entry.insert(Arc::new(RepoClient::new(
                    repo,
                    config,
                    self.client.clone(),
                    self.user_cache.clone(),
                    DefaultMergeWorkflow,
                )));
            }
        }
        Ok(())
    }

    pub async fn fetch_repositories(self: &Arc<Self>) -> anyhow::Result<()> {
        let mut repositories = Vec::new();
        let mut page = 1;
        loop {
            let resp: InstallationRepositories = self
                .client
                .get(format!("/installation/repositories?per_page=100&page={page}"), None::<&()>)
                .await
                .context("get installation repositories")?;

            repositories.extend(resp.repositories);

            if repositories.len() >= resp.total_count as usize {
                break;
            }

            page += 1;
        }

        let mut repos = HashMap::new();
        for repo in repositories {
            let config = self.get_repo_config(repo.id).await?;
            repos.insert(
                repo.id,
                Arc::new(RepoClient::new(
                    repo.into(),
                    config,
                    self.client.clone(),
                    self.user_cache.clone(),
                    DefaultMergeWorkflow,
                )),
            );
        }

        *self.repositories.lock() = repos;

        Ok(())
    }

    pub fn repositories(&self) -> Vec<RepositoryId> {
        self.repositories.lock().keys().cloned().collect()
    }

    pub fn has_repository(&self, repo_id: RepositoryId) -> bool {
        self.repositories.lock().contains_key(&repo_id)
    }

    pub fn get_repo_client(&self, repo_id: RepositoryId) -> Option<Arc<RepoClient>> {
        self.repositories.lock().get(&repo_id).cloned()
    }

    pub async fn fetch_repository(self: &Arc<Self>, id: RepositoryId) -> anyhow::Result<()> {
        let repo = self.client.repos_by_id(id).get().await.context("get repository")?;
        self.set_repository(repo.into()).await.context("set repository")?;
        Ok(())
    }

    pub fn remove_repository(&self, repo_id: RepositoryId) {
        self.repositories.lock().remove(&repo_id);
    }

    pub fn installation(&self) -> Installation {
        self.installation.lock().clone()
    }

    pub fn owner(&self) -> String {
        self.installation.lock().account.login.clone()
    }

    pub fn update_installation(&self, installation: Installation) {
        tracing::info!("updated installation: {} ({})", installation.account.login, installation.id);
        *self.installation.lock() = installation;
    }
}

impl UserCache {
    pub async fn get_user(&self, client: &Octocrab, user_id: UserId) -> anyhow::Result<Option<User>> {
        self.users
            .try_get_with::<_, octocrab::Error>(user_id, async {
                let user = match client.users_by_id(user_id).profile().await {
                    Ok(user) => user,
                    Err(octocrab::Error::GitHub {
                        source:
                            GitHubError {
                                status_code: http::StatusCode::NOT_FOUND,
                                ..
                            },
                        ..
                    }) => return Ok(None),
                    Err(e) => return Err(e),
                };
                self.users_by_name.insert(user.login.to_lowercase(), Some(user_id)).await;
                Ok(Some(user.into()))
            })
            .await
            .context("get user profile")
    }

    pub async fn get_user_by_name(&self, client: &Octocrab, name: &str) -> anyhow::Result<Option<User>> {
        let user_id = self
            .users_by_name
            .try_get_with::<_, octocrab::Error>(name.trim_start_matches('@').to_lowercase(), async {
                let user = match client.users(name).profile().await {
                    Ok(user) => user,
                    Err(octocrab::Error::GitHub {
                        source:
                            GitHubError {
                                status_code: http::StatusCode::NOT_FOUND,
                                ..
                            },
                        ..
                    }) => return Ok(None),
                    Err(e) => return Err(e),
                };
                let user_id = user.id;
                self.users.insert(user_id, Some(user.into())).await;
                Ok(Some(user_id))
            })
            .await
            .context("get user by name")?;

        if let Some(user_id) = user_id {
            self.get_user(client, user_id).await
        } else {
            Ok(None)
        }
    }

    pub async fn get_team_users(&self, client: &Octocrab, owner: &str, team: &str) -> anyhow::Result<Vec<UserId>> {
        self.teams
            .try_get_with_by_ref::<_, octocrab::Error, _>(team, async {
                let team = match client.teams(owner).members(team).per_page(100).send().await {
                    Ok(team) => team,
                    Err(octocrab::Error::GitHub {
                        source:
                            GitHubError {
                                status_code: http::StatusCode::NOT_FOUND,
                                ..
                            },
                        ..
                    }) => {
                        tracing::info!("team not found: {}/{}", owner, team);
                        return Ok(Vec::new());
                    }
                    Err(e) => return Err(e),
                };

                let users = team.into_stream(client).try_collect::<Vec<_>>().await?;
                Ok(users.into_iter().map(|u| u.id).collect())
            })
            .await
            .context("get team users")
    }
}
