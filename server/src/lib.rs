#![cfg_attr(coverage_nightly, feature(coverage_attribute))]

use database::DatabaseConnection;
use github::repo::GitHubRepoClient;
use octocrab::models::{InstallationId, RepositoryId};

pub mod auto_start;
pub mod command;
pub mod database;
pub mod github;
pub mod migrations;
pub mod webhook;

mod utils;

pub trait BrawlState: Send + Sync + 'static {
    fn get_repo(
        &self,
        installation_id: Option<InstallationId>,
        repo_id: RepositoryId,
    ) -> impl std::future::Future<Output = Option<impl GitHubRepoClient + 'static>> + Send;

    fn database(&self) -> impl std::future::Future<Output = anyhow::Result<impl DatabaseConnection + Send + '_>> + Send;
}
