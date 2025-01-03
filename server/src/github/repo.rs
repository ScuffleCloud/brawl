use std::fmt::Debug;
use std::marker::PhantomData;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use arc_swap::{ArcSwap, ArcSwapOption};
use axum::http;
use moka::future::Cache;
use octocrab::models::repos::{Object, Ref};
use octocrab::models::{RepositoryId, RunId, UserId};
use octocrab::params::repos::Reference;
use octocrab::params::{self};
use octocrab::{GitHubError, Octocrab};

use super::config::{GitHubBrawlRepoConfig, Permission, Role};
use super::installation::OrgState;
use super::merge_workflow::{DefaultMergeWorkflow, GitHubMergeWorkflow};
use super::messages::{CommitMessage, IssueMessage};
use super::models::{Commit, Label, PullRequest, Repository, Review, User, WorkflowRun};

pub struct RepoClient<W = DefaultMergeWorkflow> {
    pub(super) repo: ArcSwap<Repository>,
    pub(super) base_commit_sha: ArcSwapOption<String>,
    configs: Cache<String, Option<Arc<GitHubBrawlRepoConfig>>>,
    client: Octocrab,
    org_state: OrgState,
    merge_workflow: W,
    role_users: Cache<Role, Vec<UserId>>,
}

pub struct RepoClientRef<T, C>(T, PhantomData<C>);

impl<T, C> RepoClientRef<T, C>
where
    T: AsRef<C>,
    C: GitHubRepoClient,
{
    pub fn new(client: T) -> Self {
        Self(client, PhantomData)
    }
}

// Helper macro to forward all methods from the inner client
macro_rules! forward_fns {
    (
        $(fn $fn:ident(&self $(,$arg:ident: $arg_ty:ty)*$(,)?) -> $ret:ty;)*
    ) => {
        $(
            #[cfg_attr(all(test, coverage_nightly), coverage(off))]
            fn $fn(&self $(,$arg: $arg_ty)*) -> $ret {
                self.0.as_ref().$fn($($arg),*)
            }
        )*
    };
}

impl<T, C> GitHubRepoClient for RepoClientRef<T, C>
where
    T: AsRef<C> + Send + Sync,
    C: GitHubRepoClient,
{
    type MergeWorkflow<'a>
        = C::MergeWorkflow<'a>
    where
        C: 'a,
        T: 'a;

    // Forward all methods from the inner client
    forward_fns! {
        fn id(&self) -> RepositoryId;
        fn add_labels(&self, issue_number: u64, labels: &[String]) -> impl std::future::Future<Output = anyhow::Result<Vec<Label>>> + Send;
        fn remove_label(&self, issue_number: u64, labels: &str) -> impl std::future::Future<Output = anyhow::Result<Vec<Label>>> + Send;
        fn branch_workflows(&self, branch: &str) -> impl std::future::Future<Output = anyhow::Result<Vec<WorkflowRun>>> + Send;
        fn can_merge(&self, user_id: UserId) -> impl std::future::Future<Output = anyhow::Result<bool>> + Send;
        fn can_try(&self, user_id: UserId) -> impl std::future::Future<Output = anyhow::Result<bool>> + Send;
        fn can_review(&self, user_id: UserId) -> impl std::future::Future<Output = anyhow::Result<bool>> + Send;
        fn config(&self) -> impl std::future::Future<Output = anyhow::Result<Option<Arc<GitHubBrawlRepoConfig>>>> + Send;
        fn config_at_commit(&self, sha: &str) -> impl std::future::Future<Output = anyhow::Result<Option<Arc<GitHubBrawlRepoConfig>>>> + Send;
        fn base_commit_sha(&self) -> Option<Arc<String>>;
        fn owner(&self) -> String;
        fn name(&self) -> String;
        fn merge_workflow(&self) -> Self::MergeWorkflow<'_>;
        fn cancel_workflow_run(&self, run_id: RunId) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;
        fn has_permission(&self, user_id: UserId, permissions: &[Permission]) -> impl std::future::Future<Output = anyhow::Result<bool>> + Send;
        fn get_user(&self, user_id: UserId) -> impl std::future::Future<Output = anyhow::Result<Option<User>>> + Send;
        fn create_commit(&self, message: String, parents: Vec<String>, tree: String) -> impl std::future::Future<Output = anyhow::Result<Commit>> + Send;
        fn get_ref_latest_commit(&self, gh_ref: &params::repos::Reference) -> impl std::future::Future<Output = anyhow::Result<Option<Commit>>> + Send;
        fn push_branch(&self, branch: &str, sha: &str, force: bool) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;
        fn delete_branch(&self, branch: &str) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;
        fn get_pull_request(&self, number: u64) -> impl std::future::Future<Output = anyhow::Result<PullRequest>> + Send;
        fn get_role_members(&self, role: Role) -> impl std::future::Future<Output = anyhow::Result<Vec<UserId>>> + Send;
        fn get_reviewers(&self, pr_number: u64) -> impl std::future::Future<Output = anyhow::Result<Vec<Review>>> + Send;
        fn send_message(&self, issue_number: u64, message: &IssueMessage) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;
        fn get_commit(&self, sha: &str) -> impl std::future::Future<Output = anyhow::Result<Option<Commit>>> + Send;
        fn create_merge(&self, message: &CommitMessage, base_sha: &str, head_sha: &str, config: &GitHubBrawlRepoConfig) -> impl std::future::Future<Output = anyhow::Result<MergeResult>> + Send;
        fn commit_link(&self, sha: &str) -> String;
        fn workflow_run_link(&self, run_id: RunId) -> String;
        fn pr_link(&self, pr_number: u64) -> String;
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeResult {
    Success(Commit),
    Conflict,
}

pub trait GitHubRepoClient: Send + Sync {
    type MergeWorkflow<'a>: GitHubMergeWorkflow
    where
        Self: 'a;

    /// The ID of the repository
    fn id(&self) -> RepositoryId;

    /// The repository configuration (default branch)
    fn config(&self) -> impl std::future::Future<Output = anyhow::Result<Option<Arc<GitHubBrawlRepoConfig>>>> + Send {
        async move {
            let Some(sha) = self.base_commit_sha() else {
                return Ok(None);
            };

            self.config_at_commit(&sha).await
        }
    }

    fn config_at_commit(&self, sha: &str) -> impl std::future::Future<Output = anyhow::Result<Option<Arc<GitHubBrawlRepoConfig>>>> + Send;

    /// The base commit SHA
    fn base_commit_sha(&self) -> Option<Arc<String>>;

    /// The owner of the repository
    /// The owner of the repository
    fn owner(&self) -> String;

    /// The name of the repository
    fn name(&self) -> String;

    /// The merge workflow for this client
    fn merge_workflow(&self) -> Self::MergeWorkflow<'_>;

    /// A link to a pull request in the repository
    fn pr_link(&self, pr_number: u64) -> String {
        format!(
            "https://github.com/{owner}/{repo}/pull/{pr_number}",
            owner = self.owner(),
            repo = self.name(),
            pr_number = pr_number,
        )
    }

    /// A link to a commit in the repository
    fn commit_link(&self, sha: &str) -> String {
        format!(
            "https://github.com/{owner}/{repo}/commit/{sha}",
            owner = self.owner(),
            repo = self.name(),
            sha = sha,
        )
    }

    /// A link to a workflow run in the repository
    fn workflow_run_link(&self, run_id: RunId) -> String {
        format!(
            "https://github.com/{owner}/{repo}/actions/runs/{run_id}",
            owner = self.owner(),
            repo = self.name(),
            run_id = run_id,
        )
    }

    /// Get a user by their ID
    fn get_user(&self, user_id: UserId) -> impl std::future::Future<Output = anyhow::Result<Option<User>>> + Send;

    /// Get a pull request by its number
    fn get_pull_request(&self, number: u64) -> impl std::future::Future<Output = anyhow::Result<PullRequest>> + Send;

    /// Get the members of a role
    fn get_role_members(&self, role: Role) -> impl std::future::Future<Output = anyhow::Result<Vec<UserId>>> + Send;

    /// Get the reviewers of a pull request
    fn get_reviewers(&self, pr_number: u64) -> impl std::future::Future<Output = anyhow::Result<Vec<Review>>> + Send;

    /// Send a message to a pull request
    fn send_message(
        &self,
        issue_number: u64,
        message: &IssueMessage,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;

    /// Get a commit by its SHA
    fn get_commit(&self, sha: &str) -> impl std::future::Future<Output = anyhow::Result<Option<Commit>>> + Send;

    /// Create a merge commit
    fn create_merge(
        &self,
        message: &CommitMessage,
        base_sha: &str,
        head_sha: &str,
        config: &GitHubBrawlRepoConfig,
    ) -> impl std::future::Future<Output = anyhow::Result<MergeResult>> + Send;

    /// Create a commit
    fn create_commit(
        &self,
        message: String,
        parents: Vec<String>,
        tree: String,
    ) -> impl std::future::Future<Output = anyhow::Result<Commit>> + Send;

    /// Get a reference by its name
    fn get_ref_latest_commit(
        &self,
        gh_ref: &params::repos::Reference,
    ) -> impl std::future::Future<Output = anyhow::Result<Option<Commit>>> + Send;

    /// Push a branch to the repository
    fn push_branch(
        &self,
        branch: &str,
        sha: &str,
        force: bool,
    ) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;

    /// Delete a branch from the repository
    fn delete_branch(&self, branch: &str) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;

    /// Get the workflows for a branch
    fn branch_workflows(&self, branch: &str) -> impl std::future::Future<Output = anyhow::Result<Vec<WorkflowRun>>> + Send;

    /// Cancel a workflow
    fn cancel_workflow_run(&self, run_id: RunId) -> impl std::future::Future<Output = anyhow::Result<()>> + Send;

    /// Add labels to a pull request
    fn add_labels(
        &self,
        issue_number: u64,
        labels: &[String],
    ) -> impl std::future::Future<Output = anyhow::Result<Vec<Label>>> + Send;

    /// Remove a label from a pull request
    fn remove_label(
        &self,
        issue_number: u64,
        labels: &str,
    ) -> impl std::future::Future<Output = anyhow::Result<Vec<Label>>> + Send;

    /// Check if a user has a permission
    fn has_permission(
        &self,
        user_id: UserId,
        permissions: &[Permission],
    ) -> impl std::future::Future<Output = anyhow::Result<bool>> + Send;

    /// Check if a user can merge
    fn can_merge(&self, user_id: UserId) -> impl std::future::Future<Output = anyhow::Result<bool>> + Send {
        async move {
            let Some(config) = self.config().await? else {
                return Ok(false);
            };

            self.has_permission(user_id, &config.merge_permissions).await
        }
    }

    /// Check if a user can try
    fn can_try(&self, user_id: UserId) -> impl std::future::Future<Output = anyhow::Result<bool>> + Send {
        async move {
            let Some(config) = self.config().await? else {
                return Ok(false);
            };

            self.has_permission(user_id, config.try_permissions()).await
        }
    }

    /// Check if a user can review
    fn can_review(&self, user_id: UserId) -> impl std::future::Future<Output = anyhow::Result<bool>> + Send {
        async move {
            let Some(config) = self.config().await? else {
                return Ok(false);
            };

            self.has_permission(user_id, config.reviewer_permissions()).await
        }
    }
}

impl<W: GitHubMergeWorkflow> RepoClient<W> {
    pub(super) fn new(
        repo: Repository,
        client: Octocrab,
        user_cache: OrgState,
        merge_workflow: W,
        base_commit_sha: Option<String>,
    ) -> Self {
        tracing::info!(
            id = %repo.id,
            name = %repo.name,
            owner = %repo.owner.login,
            "new repo loaded"
        );

        Self {
            repo: ArcSwap::from_pointee(repo),
            configs: Cache::builder()
                .max_capacity(100)
                .time_to_idle(Duration::from_secs(600))
                .time_to_live(Duration::from_secs(24 * 60 * 60))
                .build(),
            client,
            org_state: user_cache,
            merge_workflow,
            role_users: Cache::builder()
                .max_capacity(50)
                .time_to_live(Duration::from_secs(60))
                .build(),
            base_commit_sha: ArcSwapOption::from_pointee(base_commit_sha),
        }
    }
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


impl<W: GitHubMergeWorkflow> GitHubRepoClient for RepoClient<W> {
    type MergeWorkflow<'a>
        = &'a W
    where
        W: 'a;

    fn id(&self) -> RepositoryId {
        self.repo.load().id
    }

    fn merge_workflow(&self) -> Self::MergeWorkflow<'_> {
        &self.merge_workflow
    }

    async fn config_at_commit(&self, sha: &str) -> anyhow::Result<Option<Arc<GitHubBrawlRepoConfig>>> {
        let config = self.configs.try_get_with_by_ref::<_, GitHubBrawlRepoConfigError, _>(sha, async {
            let file = match self
                .client
                .repos_by_id(self.id())
                .get_content()
                .path(".github/brawl.toml")
                .r#ref(sha)
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
                    return Ok(None);
                }
                Err(e) => return Err(e.into()),
            };
    
            if file.items.is_empty() {
                return Ok(None);
            }
    
            if file.items.len() != 1 {
                return Err(GitHubBrawlRepoConfigError::ExpectedOneFile(file.items.len()));
            }
    
            let config = toml::from_str(
                &file.items[0]
                    .decoded_content()
                    .ok_or(GitHubBrawlRepoConfigError::MissingContent)?,
            )?;
    
            Ok(Some(Arc::new(config)))
        }).await?;

        Ok(config)
    }

    fn base_commit_sha(&self) -> Option<Arc<String>> {
        self.base_commit_sha.load_full()
    }

    fn name(&self) -> String {
        self.repo.load().name.clone()
    }

    fn owner(&self) -> String {
        self.repo.load().owner.login.clone()
    }

    async fn get_user(&self, user_id: UserId) -> anyhow::Result<Option<User>> {
        self.org_state.get_user(&self.client, user_id).await
    }

    async fn get_pull_request(&self, number: u64) -> anyhow::Result<PullRequest> {
        self.client
            .pulls(self.owner(), self.name())
            .get(number)
            .await
            .context("get pull request")
            .map(|p| p.into())
    }

    async fn get_role_members(&self, role: Role) -> anyhow::Result<Vec<UserId>> {
        self.role_users
            .try_get_with::<_, octocrab::Error>(role, async {
                let users = self
                    .client
                    .repos_by_id(self.id())
                    .list_collaborators()
                    .permission(role.into())
                    .send()
                    .await?;

                Ok(users.items.into_iter().map(|c| c.author.id).collect())
            })
            .await
            .context("get role members")
    }

    async fn send_message(&self, issue_number: u64, message: &IssueMessage) -> anyhow::Result<()> {
        self.client
            .issues_by_id(self.id())
            .create_comment(issue_number, message)
            .await
            .context("send message")?;
        Ok(())
    }

    async fn create_merge(&self, message: &CommitMessage, base_sha: &str, head_sha: &str, config: &GitHubBrawlRepoConfig) -> anyhow::Result<MergeResult> {
        let tmp_branch = config.temp_branch();

        self.push_branch(&tmp_branch, base_sha, true)
            .await
            .context("push tmp branch")?;

        let commit = match self
            .client
            .post::<_, octocrab::models::commits::Commit>(
                format!("/repos/{owner}/{repo}/merges", owner = self.owner(), repo = self.name()),
                Some(&serde_json::json!({
                    "base": tmp_branch,
                    "head": head_sha,
                    "commit_message": message.as_ref(),
                })),
            )
            .await
        {
            Ok(c) => Ok(MergeResult::Success(c.into())),
            Err(octocrab::Error::GitHub {
                source:
                    GitHubError {
                        status_code: http::StatusCode::CONFLICT,
                        ..
                    },
                ..
            }) => Ok(MergeResult::Conflict),
            Err(e) => Err(e).context("create merge"),
        };

        if let Err(e) = self.delete_branch(&tmp_branch).await {
            tracing::error!("failed to delete tmp branch: {:#}", e);
        }

        commit
    }

    async fn create_commit(&self, message: String, parents: Vec<String>, tree: String) -> anyhow::Result<Commit> {
        self.client
            .repos_by_id(self.id())
            .create_git_commit_object(message, tree)
            .parents(parents)
            .send()
            .await
            .context("create commit")
            .map(|c| c.into())
    }

    async fn push_branch(&self, branch: &str, sha: &str, force: bool) -> anyhow::Result<()> {
        let branch_ref = Reference::Branch(branch.to_owned());

        if let Some(current_ref) = self.get_ref_latest_commit(&branch_ref).await? {
            if current_ref.sha == sha {
                return Ok(());
            }
        } else {
            self.client
                .repos_by_id(self.id())
                .create_ref(&branch_ref, sha)
                .await
                .context("create ref")?;

            return Ok(());
        }

        self.client
            .patch::<Ref, _, _>(
                format!(
                    "/repos/{owner}/{repo}/git/refs/heads/{branch}",
                    owner = self.owner(),
                    repo = self.name(),
                    branch = branch
                ),
                Some(&serde_json::json!({
                    "sha": sha,
                    "force": force,
                })),
            )
            .await
            .context("update ref")?;

        Ok(())
    }

    async fn delete_branch(&self, branch: &str) -> anyhow::Result<()> {
        match self
            .client
            .repos_by_id(self.id())
            .delete_ref(&Reference::Branch(branch.to_owned()))
            .await
        {
            Ok(_) => Ok(()),
            Err(octocrab::Error::GitHub {
                source:
                    GitHubError {
                        status_code: http::StatusCode::UNPROCESSABLE_ENTITY,
                        message,
                        ..
                    },
                ..
            }) if message == "Reference does not exist" => Ok(()),
            Err(e) => Err(e).context("delete branch"),
        }
    }

    async fn add_labels(&self, issue_number: u64, labels: &[String]) -> anyhow::Result<Vec<Label>> {
        self.client
            .issues_by_id(self.id())
            .add_labels(issue_number, labels)
            .await
            .context("add labels")
            .map(|labels| labels.into_iter().map(Label::from).collect())
    }

    async fn remove_label(&self, issue_number: u64, label: &str) -> anyhow::Result<Vec<Label>> {
        self.client
            .issues_by_id(self.id())
            .remove_label(issue_number, label)
            .await
            .context("remove label")
            .map(|labels| labels.into_iter().map(Label::from).collect())
    }

    async fn get_commit(&self, sha: &str) -> anyhow::Result<Option<Commit>> {
        match self
            .client
            .get::<octocrab::models::commits::Commit, _, _>(
                format!(
                    "/repos/{owner}/{repo}/commits/{sha}",
                    owner = self.owner(),
                    repo = self.name(),
                    sha = sha,
                ),
                None::<&()>,
            )
            .await
        {
            Ok(commit) => Ok(Some(commit.into())),
            Err(octocrab::Error::GitHub {
                source:
                    GitHubError {
                        status_code: http::StatusCode::NOT_FOUND,
                        ..
                    },
                ..
            }) => Ok(None),
            Err(e) => Err(e).context("get commit by sha"),
        }
    }

    async fn get_ref_latest_commit(&self, gh_ref: &params::repos::Reference) -> anyhow::Result<Option<Commit>> {
        match self.client.repos_by_id(self.id()).get_ref(gh_ref).await {
            Ok(r) => Ok(Some(match r.object {
                Object::Commit { sha, .. } => Commit { sha, tree: None },
                Object::Tag { sha, .. } => Commit { sha, tree: None },
                _ => return Err(anyhow::anyhow!("invalid object type")),
            })),
            Err(octocrab::Error::GitHub {
                source:
                    GitHubError {
                        status_code: http::StatusCode::NOT_FOUND,
                        ..
                    },
                ..
            }) => Ok(None),
            Err(e) => Err(e).context("get ref"),
        }
    }

    async fn has_permission(&self, user_id: UserId, permissions: &[Permission]) -> anyhow::Result<bool> {
        let owner = self.owner();
        for permission in permissions {
            match permission {
                Permission::Role(role) => {
                    let users = self.get_role_members(*role).await?;
                    if users.contains(&user_id) {
                        return Ok(true);
                    }
                }
                Permission::Team(team) => {
                    let users = self.org_state.get_team_users(&self.client, &owner, team).await?;
                    if users.contains(&user_id) {
                        return Ok(true);
                    }
                }
                Permission::User(user) => {
                    if let Some(user) = self.org_state.get_user_by_name(&self.client, user).await? {
                        if user.id == user_id {
                            return Ok(true);
                        }
                    }
                }
            }
        }

        Ok(false)
    }

    async fn get_reviewers(&self, pr_number: u64) -> anyhow::Result<Vec<Review>> {
        let reviewers = self
            .client
            .all_pages(
                self.client
                    .pulls(self.owner(), self.name())
                    .list_reviews(pr_number)
                    .per_page(100)
                    .send()
                    .await?,
            )
            .await?;
        Ok(reviewers.into_iter().map(|r| r.into()).collect())
    }

    async fn branch_workflows(&self, branch: &str) -> anyhow::Result<Vec<WorkflowRun>> {
        let page = self
            .client
            .workflows(self.owner(), self.name())
            .list_all_runs()
            .branch(branch.to_owned())
            .per_page(100)
            .page(1u32)
            .send()
            .await?;

        let pages = self.client.all_pages(page).await?;

        Ok(pages.into_iter().map(|w| w.into()).collect())
    }

    async fn cancel_workflow_run(&self, run_id: RunId) -> anyhow::Result<()> {
        self.client
            .post::<_, serde_json::Value>(
                format!(
                    "/repos/{owner}/{repo}/actions/runs/{id}/cancel",
                    owner = self.owner(),
                    repo = self.name(),
                    id = run_id
                ),
                None::<&()>,
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
pub mod test_utils {
    use super::*;

    #[derive(Debug)]
    pub struct MockRepoClient<T: GitHubMergeWorkflow> {
        pub id: RepositoryId,
        pub owner: String,
        pub name: String,
        pub config: Option<GitHubBrawlRepoConfig>,
        pub base_commit_sha: Option<String>,
        pub actions: tokio::sync::mpsc::Sender<MockRepoAction>,
        pub merge_workflow: T,
    }

    impl<T: GitHubMergeWorkflow> MockRepoClient<T> {
        pub fn new(merge_workflow: T) -> (Self, tokio::sync::mpsc::Receiver<MockRepoAction>) {
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            (
                Self {
                    id: RepositoryId(1),
                    owner: "owner".to_owned(),
                    name: "repo".to_owned(),
                    config: Some(GitHubBrawlRepoConfig::default()),
                    base_commit_sha: Some("base".to_owned()),
                    actions: tx,
                    merge_workflow,
                },
                rx,
            )
        }

        pub fn with_id(self, id: RepositoryId) -> Self {
            Self { id, ..self }
        }

        pub fn with_config(self, config: impl Into<Option<GitHubBrawlRepoConfig>>) -> Self {
            Self { config: config.into(), ..self }
        }

        pub fn with_base_commit_sha(self, base_commit_sha: impl Into<Option<String>>) -> Self {
            Self { base_commit_sha: base_commit_sha.into(), ..self }
        }

        pub fn with_owner(self, owner: String) -> Self {
            Self { owner, ..self }
        }

        pub fn with_name(self, name: String) -> Self {
            Self { name, ..self }
        }
    }

    #[derive(Debug)]
    pub enum MockRepoAction {
        GetUser {
            user_id: UserId,
            result: tokio::sync::oneshot::Sender<anyhow::Result<Option<User>>>,
        },
        GetPullRequest {
            number: u64,
            result: tokio::sync::oneshot::Sender<anyhow::Result<PullRequest>>,
        },
        SetPullRequest {
            pull_request: Box<PullRequest>,
        },
        GetRoleMembers {
            role: Role,
            result: tokio::sync::oneshot::Sender<anyhow::Result<Vec<UserId>>>,
        },
        GetReviewers {
            pr_number: u64,
            result: tokio::sync::oneshot::Sender<anyhow::Result<Vec<Review>>>,
        },
        SendMessage {
            issue_number: u64,
            message: IssueMessage,
            result: tokio::sync::oneshot::Sender<anyhow::Result<()>>,
        },
        GetCommit {
            sha: String,
            result: tokio::sync::oneshot::Sender<anyhow::Result<Option<Commit>>>,
        },
        CreateMerge {
            message: CommitMessage,
            base_sha: String,
            head_sha: String,
            config: GitHubBrawlRepoConfig,
            result: tokio::sync::oneshot::Sender<anyhow::Result<MergeResult>>,
        },
        CreateCommit {
            message: String,
            parents: Vec<String>,
            tree: String,
            result: tokio::sync::oneshot::Sender<anyhow::Result<Commit>>,
        },
        GetRefLatestCommit {
            gh_ref: params::repos::Reference,
            result: tokio::sync::oneshot::Sender<anyhow::Result<Option<Commit>>>,
        },
        PushBranch {
            branch: String,
            sha: String,
            force: bool,
            result: tokio::sync::oneshot::Sender<anyhow::Result<()>>,
        },
        DeleteBranch {
            branch: String,
            result: tokio::sync::oneshot::Sender<anyhow::Result<()>>,
        },
        HasPermission {
            user_id: UserId,
            permissions: Vec<Permission>,
            result: tokio::sync::oneshot::Sender<anyhow::Result<bool>>,
        },
        AddLabels {
            issue_number: u64,
            labels: Vec<String>,
            result: tokio::sync::oneshot::Sender<anyhow::Result<Vec<Label>>>,
        },
        RemoveLabel {
            issue_number: u64,
            label: String,
            result: tokio::sync::oneshot::Sender<anyhow::Result<Vec<Label>>>,
        },
        BranchWorkflows {
            branch: String,
            result: tokio::sync::oneshot::Sender<anyhow::Result<Vec<WorkflowRun>>>,
        },
        CancelWorkflowRun {
            run_id: RunId,
            result: tokio::sync::oneshot::Sender<anyhow::Result<()>>,
        },
        GetConfigAtCommit {
            sha: String,
            result: tokio::sync::oneshot::Sender<anyhow::Result<Option<Arc<GitHubBrawlRepoConfig>>>>,
        },
    }

    impl<T: GitHubMergeWorkflow> GitHubRepoClient for MockRepoClient<T> {
        type MergeWorkflow<'a>
            = &'a T
        where
            T: 'a;

        fn merge_workflow(&self) -> Self::MergeWorkflow<'_> {
            &self.merge_workflow
        }

        fn id(&self) -> RepositoryId {
            self.id
        }

        async fn config(&self) -> anyhow::Result<Option<Arc<GitHubBrawlRepoConfig>>> {
            let Some(config) = self.config.clone() else {
                if let Some(base_commit_sha) = self.base_commit_sha.as_deref() {
                    return self.config_at_commit(base_commit_sha).await;
                }

                return Ok(None);
            };

            Ok(Some(Arc::new(config)))
        }

        fn base_commit_sha(&self) -> Option<Arc<String>> {
            self.base_commit_sha.clone().map(Arc::new)
        }

        fn owner(&self) -> String {
            self.owner.clone()
        }

        fn name(&self) -> String {
            self.name.clone()
        }

        async fn get_user(&self, user_id: UserId) -> anyhow::Result<Option<User>> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.actions
                .send(MockRepoAction::GetUser { user_id, result: tx })
                .await
                .expect("send get user");
            rx.await.expect("recv get user")
        }

        async fn get_pull_request(&self, number: u64) -> anyhow::Result<PullRequest> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.actions
                .send(MockRepoAction::GetPullRequest { number, result: tx })
                .await
                .expect("send get pull request");
            rx.await.expect("recv get pull request")
        }

        async fn get_role_members(&self, role: Role) -> anyhow::Result<Vec<UserId>> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.actions
                .send(MockRepoAction::GetRoleMembers { role, result: tx })
                .await
                .expect("send get role members");
            rx.await.expect("recv get role members")
        }

        async fn get_reviewers(&self, pr_number: u64) -> anyhow::Result<Vec<Review>> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.actions
                .send(MockRepoAction::GetReviewers { pr_number, result: tx })
                .await
                .expect("send get reviewers");
            rx.await.expect("recv get reviewers")
        }

        async fn send_message(&self, issue_number: u64, message: &IssueMessage) -> anyhow::Result<()> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.actions
                .send(MockRepoAction::SendMessage {
                    issue_number,
                    message: message.clone(),
                    result: tx,
                })
                .await
                .expect("send send message");
            rx.await.expect("recv send message")
        }

        async fn get_commit(&self, sha: &str) -> anyhow::Result<Option<Commit>> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.actions
                .send(MockRepoAction::GetCommit {
                    sha: sha.to_string(),
                    result: tx,
                })
                .await
                .expect("send get commit");
            rx.await.expect("recv get commit")
        }

        async fn create_merge(
            &self,
            message: &CommitMessage,
            base_sha: &str,
            head_sha: &str,
            config: &GitHubBrawlRepoConfig,
        ) -> anyhow::Result<MergeResult> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.actions
                .send(MockRepoAction::CreateMerge {
                    message: message.clone(),
                    base_sha: base_sha.to_string(),
                    head_sha: head_sha.to_string(),
                    config: config.clone(),
                    result: tx,
                })
                .await
                .expect("send create merge");
            rx.await.expect("recv create merge")
        }

        async fn create_commit(&self, message: String, parents: Vec<String>, tree: String) -> anyhow::Result<Commit> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.actions
                .send(MockRepoAction::CreateCommit {
                    message,
                    parents,
                    tree,
                    result: tx,
                })
                .await
                .expect("send create commit");
            rx.await.expect("recv create commit")
        }

        async fn get_ref_latest_commit(&self, gh_ref: &params::repos::Reference) -> anyhow::Result<Option<Commit>> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.actions
                .send(MockRepoAction::GetRefLatestCommit {
                    gh_ref: gh_ref.clone(),
                    result: tx,
                })
                .await
                .expect("send get ref latest commit");
            rx.await.expect("recv get ref latest commit")
        }

        async fn push_branch(&self, branch: &str, sha: &str, force: bool) -> anyhow::Result<()> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.actions
                .send(MockRepoAction::PushBranch {
                    branch: branch.to_string(),
                    sha: sha.to_string(),
                    force,
                    result: tx,
                })
                .await
                .expect("send push branch");
            rx.await.expect("recv push branch")
        }

        async fn delete_branch(&self, branch: &str) -> anyhow::Result<()> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.actions
                .send(MockRepoAction::DeleteBranch {
                    branch: branch.to_string(),
                    result: tx,
                })
                .await
                .expect("send delete branch");
            rx.await.expect("recv delete branch")
        }

        async fn has_permission(&self, user_id: UserId, permissions: &[Permission]) -> anyhow::Result<bool> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.actions
                .send(MockRepoAction::HasPermission {
                    user_id,
                    permissions: permissions.to_vec(),
                    result: tx,
                })
                .await
                .expect("send has permission");
            rx.await.expect("recv has permission")
        }

        async fn add_labels(&self, issue_number: u64, labels: &[String]) -> anyhow::Result<Vec<Label>> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.actions
                .send(MockRepoAction::AddLabels {
                    issue_number,
                    labels: labels.to_vec(),
                    result: tx,
                })
                .await
                .expect("send add labels");
            rx.await.expect("recv add labels")
        }

        async fn remove_label(&self, issue_number: u64, label: &str) -> anyhow::Result<Vec<Label>> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.actions
                .send(MockRepoAction::RemoveLabel {
                    issue_number,
                    label: label.to_owned(),
                    result: tx,
                })
                .await
                .expect("send remove label");
            rx.await.expect("recv remove label")
        }

        async fn branch_workflows(&self, branch: &str) -> anyhow::Result<Vec<WorkflowRun>> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.actions
                .send(MockRepoAction::BranchWorkflows {
                    branch: branch.to_string(),
                    result: tx,
                })
                .await
                .expect("send branch workflows");
            rx.await.expect("recv branch workflows")
        }

        async fn cancel_workflow_run(&self, run_id: RunId) -> anyhow::Result<()> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.actions
                .send(MockRepoAction::CancelWorkflowRun { run_id, result: tx })
                .await
                .expect("send cancel workflow");
            rx.await.expect("recv cancel workflow")
        }

        async fn config_at_commit(&self, sha: &str) -> anyhow::Result<Option<Arc<GitHubBrawlRepoConfig>>> {
            let (tx, rx) = tokio::sync::oneshot::channel();
            self.actions
                .send(MockRepoAction::GetConfigAtCommit { sha: sha.to_string(), result: tx })
                .await
                .expect("send get config at commit");
            rx.await.expect("recv get config at commit")
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use http::{Method, StatusCode};

    use super::*;
    use crate::github::messages;
    use crate::github::test_utils::{
        debug_req, default_repo, mock_octocrab, mock_repo_client, mock_response, DebugReq, MockMergeWorkflow,
    };

    #[tokio::test]
    async fn test_repo_client_accessors() {
        let (octocrab, _) = mock_octocrab();
        let repo_client = mock_repo_client(
            octocrab,
            default_repo(),
            MockMergeWorkflow(1),
            Some("base".to_owned()),
        )
        .await;

        assert_eq!(repo_client.id(), RepositoryId(899726767));
        assert_eq!(repo_client.owner(), "ScuffleCloud");
        assert_eq!(repo_client.name(), "ci-testing");
        assert_eq!(repo_client.base_commit_sha().unwrap().as_ref(), "base");
        assert_eq!(repo_client.merge_workflow().0, 1);
    }

    #[tokio::test]
    async fn test_repo_client_get_user() {
        let (octocrab, mut handle) = mock_octocrab();
        let repo_client = mock_repo_client(
            octocrab,
            default_repo(),
            MockMergeWorkflow(1),
            Some("base".to_owned()),
        )
        .await;

        let task = tokio::spawn(async move {
            let user = repo_client.get_user(UserId(49777269)).await.unwrap().unwrap();
            assert_eq!(user.id, UserId(49777269));
            assert_eq!(user.login, "TroyKomodo");

            // Second request should be cached
            let user = repo_client.get_user(UserId(49777269)).await.unwrap().unwrap();
            assert_eq!(user.id, UserId(49777269));
            assert_eq!(user.login, "TroyKomodo");

            repo_client
        });

        let (req, resp) = handle.next_request().await.unwrap();

        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: GET,
            uri: "/user/49777269",
            headers: [
                (
                    "content-length",
                    "0",
                ),
                (
                    "authorization",
                    "REDACTED",
                ),
            ],
            body: None,
        }
        "#);

        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/user.json")));

        task.await.unwrap();
    }

    #[tokio::test]
    async fn test_repo_client_get_pull_request() {
        let (octocrab, mut handle) = mock_octocrab();
        let repo_client = mock_repo_client(
            octocrab,
            default_repo(),
            MockMergeWorkflow(1),
            Some("base".to_owned()),
        )
        .await;

        let task = tokio::spawn(async move {
            let pull_request = repo_client.get_pull_request(22).await.unwrap();
            assert_eq!(pull_request.number, 22);
            assert_eq!(pull_request.title, "add fib");

            repo_client
        });

        let (req, resp) = handle.next_request().await.unwrap();

        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: GET,
            uri: "/repos/ScuffleCloud/ci-testing/pulls/22",
            headers: [
                (
                    "content-length",
                    "0",
                ),
                (
                    "authorization",
                    "REDACTED",
                ),
            ],
            body: None,
        }
        "#);

        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/pr.json")));

        task.await.unwrap();
    }

    #[tokio::test]
    async fn test_repo_client_get_role_members() {
        let (octocrab, mut handle) = mock_octocrab();
        let repo_client = mock_repo_client(
            octocrab,
            default_repo(),
            MockMergeWorkflow(1),
            Some("base".to_owned()),
        )
        .await;

        let task = tokio::spawn(async move {
            let members = repo_client.get_role_members(Role::Admin).await.unwrap();
            assert_eq!(members, vec![UserId(49777269)]);
            let members = repo_client.get_role_members(Role::Maintain).await.unwrap();
            assert_eq!(members, vec![UserId(49777269)]);
            let members = repo_client.get_role_members(Role::Pull).await.unwrap();
            assert_eq!(members, vec![UserId(49777269)]);
            let members = repo_client.get_role_members(Role::Push).await.unwrap();
            assert_eq!(members, vec![UserId(49777269)]);
            let members = repo_client.get_role_members(Role::Triage).await.unwrap();
            assert_eq!(members, vec![UserId(49777269)]);
            repo_client
        });

        type MockReq = (Box<dyn Fn(DebugReq)>, &'static [u8]);

        let cases: &[MockReq] = &[
            (
                Box::new(|req| {
                    insta::assert_debug_snapshot!(req, @r#"
                    DebugReq {
                        method: GET,
                        uri: "/repositories/899726767/collaborators?permission=admin",
                        headers: [
                            (
                                "content-length",
                                "0",
                            ),
                            (
                                "authorization",
                                "REDACTED",
                            ),
                        ],
                        body: None,
                    }
                    "#)
                }),
                include_bytes!("mock/role_members.json"),
            ),
            (
                Box::new(|req| {
                    insta::assert_debug_snapshot!(req, @r#"
                    DebugReq {
                        method: GET,
                        uri: "/repositories/899726767/collaborators?permission=maintain",
                        headers: [
                            (
                                "content-length",
                                "0",
                            ),
                            (
                                "authorization",
                                "REDACTED",
                            ),
                        ],
                        body: None,
                    }
                    "#)
                }),
                include_bytes!("mock/role_members.json"),
            ),
            (
                Box::new(|req| {
                    insta::assert_debug_snapshot!(req, @r#"
                    DebugReq {
                        method: GET,
                        uri: "/repositories/899726767/collaborators?permission=pull",
                        headers: [
                            (
                                "content-length",
                                "0",
                            ),
                            (
                                "authorization",
                                "REDACTED",
                            ),
                        ],
                        body: None,
                    }
                    "#)
                }),
                include_bytes!("mock/role_members.json"),
            ),
            (
                Box::new(|req| {
                    insta::assert_debug_snapshot!(req, @r#"
                    DebugReq {
                        method: GET,
                        uri: "/repositories/899726767/collaborators?permission=push",
                        headers: [
                            (
                                "content-length",
                                "0",
                            ),
                            (
                                "authorization",
                                "REDACTED",
                            ),
                        ],
                        body: None,
                    }
                    "#)
                }),
                include_bytes!("mock/role_members.json"),
            ),
            (
                Box::new(|req| {
                    insta::assert_debug_snapshot!(req, @r#"
                    DebugReq {
                        method: GET,
                        uri: "/repositories/899726767/collaborators?permission=triage",
                        headers: [
                            (
                                "content-length",
                                "0",
                            ),
                            (
                                "authorization",
                                "REDACTED",
                            ),
                        ],
                        body: None,
                    }
                    "#)
                }),
                include_bytes!("mock/role_members.json"),
            ),
        ];

        for (case, resp_body) in cases {
            let (req, resp) = handle.next_request().await.unwrap();
            case(debug_req(req).await);
            resp.send_response(mock_response(StatusCode::OK, resp_body));
        }

        task.await.unwrap();
    }

    #[tokio::test]
    async fn test_repo_client_send_message() {
        let (octocrab, mut handle) = mock_octocrab();
        let repo_client = mock_repo_client(
            octocrab,
            default_repo(),
            MockMergeWorkflow(1),
            Some("base".to_owned()),
        )
        .await;

        let task = tokio::spawn(async move {
            repo_client
                .send_message(22, &IssueMessage::Error("test".to_string()))
                .await
                .unwrap();
            repo_client
        });

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: POST,
            uri: "/repositories/899726767/issues/22/comments",
            headers: [
                (
                    "content-type",
                    "application/json",
                ),
                (
                    "authorization",
                    "REDACTED",
                ),
            ],
            body: Some(
                Object {
                    "body": String("test"),
                },
            ),
        }
        "#);

        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/comment.json")));

        task.await.unwrap();
    }

    #[tokio::test]
    async fn test_repo_client_get_commit() {
        let (octocrab, mut handle) = mock_octocrab();
        let repo_client = mock_repo_client(
            octocrab,
            default_repo(),
            MockMergeWorkflow(1),
            Some("base".to_owned()),
        )
        .await;

        let task = tokio::spawn(async move {
            repo_client
                .get_commit("b788e73f95c7b81b552d42fac1380c4b84b70be9")
                .await
                .unwrap();
            repo_client
        });

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: GET,
            uri: "/repos/ScuffleCloud/ci-testing/commits/b788e73f95c7b81b552d42fac1380c4b84b70be9",
            headers: [
                (
                    "content-length",
                    "0",
                ),
                (
                    "authorization",
                    "REDACTED",
                ),
            ],
            body: None,
        }
        "#);

        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/commit.json")));

        task.await.unwrap();
    }

    #[tokio::test]
    async fn test_repo_client_create_commit() {
        let (octocrab, mut handle) = mock_octocrab();
        let repo_client = mock_repo_client(
            octocrab,
            default_repo(),
            MockMergeWorkflow(1),
            Some("base".to_owned()),
        )
        .await;

        let task = tokio::spawn(async move {
            repo_client
                .create_commit(
                    "add fib".to_string(),
                    vec![
                        "7cb472a827cc492096a3ba6a6020fef5c5bbe800".to_string(),
                        "fe2e65c6ebd8b42cf98597dd5c978cca8d28773e".to_string(),
                    ],
                    "588e42e273fae85aa8849056329787c878b2b0e7".to_string(),
                )
                .await
                .unwrap();
            repo_client
        });

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: POST,
            uri: "/repositories/899726767/git/commits",
            headers: [
                (
                    "content-type",
                    "application/json",
                ),
                (
                    "authorization",
                    "REDACTED",
                ),
            ],
            body: Some(
                Object {
                    "message": String("add fib"),
                    "parents": Array [
                        String("7cb472a827cc492096a3ba6a6020fef5c5bbe800"),
                        String("fe2e65c6ebd8b42cf98597dd5c978cca8d28773e"),
                    ],
                    "tree": String("588e42e273fae85aa8849056329787c878b2b0e7"),
                },
            ),
        }
        "#);

        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/create_commit.json")));

        task.await.unwrap();
    }

    #[tokio::test]
    async fn test_repo_client_delete_branch() {
        let (octocrab, mut handle) = mock_octocrab();
        let repo_client = mock_repo_client(
            octocrab,
            default_repo(),
            MockMergeWorkflow(1),
            Some("base".to_owned()),
        )
        .await;

        let task = tokio::spawn(async move {
            repo_client.delete_branch("feature/1").await.unwrap();
            repo_client.delete_branch("non-exist").await.unwrap();
            repo_client
        });

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: DELETE,
            uri: "/repositories/899726767/git/refs/heads/feature/1",
            headers: [
                (
                    "content-length",
                    "0",
                ),
                (
                    "authorization",
                    "REDACTED",
                ),
            ],
            body: None,
        }
        "#);

        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/delete_branch.json")));

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: DELETE,
            uri: "/repositories/899726767/git/refs/heads/non-exist",
            headers: [
                (
                    "content-length",
                    "0",
                ),
                (
                    "authorization",
                    "REDACTED",
                ),
            ],
            body: None,
        }
        "#);

        resp.send_response(mock_response(
            StatusCode::OK,
            include_bytes!("mock/delete_branch_non_exist.json"),
        ));

        task.await.unwrap();
    }

    #[tokio::test]
    async fn test_repo_client_get_ref_latest_commit() {
        let (octocrab, mut handle) = mock_octocrab();
        let repo_client = mock_repo_client(
            octocrab,
            default_repo(),
            MockMergeWorkflow(1),
            Some("base".to_owned()),
        )
        .await;

        let task = tokio::spawn(async move {
            repo_client
                .get_ref_latest_commit(&params::repos::Reference::Branch("test/queue".to_string()))
                .await
                .unwrap();
            repo_client
                .get_ref_latest_commit(&params::repos::Reference::Tag("v1.0.0".to_string()))
                .await
                .unwrap();
            repo_client
                .get_ref_latest_commit(&params::repos::Reference::Tag("not-exist".to_string()))
                .await
                .unwrap();
            repo_client
        });

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: GET,
            uri: "/repositories/899726767/git/ref/heads/test/queue",
            headers: [
                (
                    "content-length",
                    "0",
                ),
                (
                    "authorization",
                    "REDACTED",
                ),
            ],
            body: None,
        }
        "#);

        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/ref_latest_commit.json")));

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: GET,
            uri: "/repositories/899726767/git/ref/tags/v1.0.0",
            headers: [
                (
                    "content-length",
                    "0",
                ),
                (
                    "authorization",
                    "REDACTED",
                ),
            ],
            body: None,
        }
        "#);

        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/ref_latest_commit.json")));

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: GET,
            uri: "/repositories/899726767/git/ref/tags/not-exist",
            headers: [
                (
                    "content-length",
                    "0",
                ),
                (
                    "authorization",
                    "REDACTED",
                ),
            ],
            body: None,
        }
        "#);

        resp.send_response(mock_response(
            StatusCode::NOT_FOUND,
            include_bytes!("mock/ref_not_found.json"),
        ));

        task.await.unwrap();
    }

    #[tokio::test]
    async fn test_repo_client_has_permission() {
        let (octocrab, mut handle) = mock_octocrab();
        let repo_client = mock_repo_client(
            octocrab,
            default_repo(),
            MockMergeWorkflow(1),
            Some("base".to_owned()),
        )
        .await;

        let task = tokio::spawn(async move {
            assert!(repo_client
                .has_permission(UserId(49777269), &[Permission::Role(Role::Admin)])
                .await
                .unwrap());
            assert!(repo_client
                .has_permission(UserId(49777269), &[Permission::Team("team1".to_string())])
                .await
                .unwrap());
            assert!(repo_client
                .has_permission(UserId(49777269), &[Permission::User("TroyKomodo".to_string())])
                .await
                .unwrap());
            assert!(!repo_client
                .has_permission(UserId(49777269), &[Permission::Team("not-exist".to_string())])
                .await
                .unwrap());
            assert!(!repo_client
                .has_permission(UserId(49777269), &[Permission::User("not-exist".to_string())])
                .await
                .unwrap());
            repo_client
        });

        type MockReq = (Box<dyn Fn(DebugReq)>, &'static [u8], StatusCode);

        let cases: &[MockReq] = &[
            (
                Box::new(|req| {
                    insta::assert_debug_snapshot!(req, @r#"
                    DebugReq {
                        method: GET,
                        uri: "/repositories/899726767/collaborators?permission=admin",
                        headers: [
                            (
                                "content-length",
                                "0",
                            ),
                            (
                                "authorization",
                                "REDACTED",
                            ),
                        ],
                        body: None,
                    }
                    "#);
                }),
                include_bytes!("mock/role_members.json"),
                StatusCode::OK,
            ),
            (
                Box::new(|req| {
                    insta::assert_debug_snapshot!(req, @r#"
                    DebugReq {
                        method: GET,
                        uri: "/orgs/scufflecloud/teams/team1/members?per_page=100",
                        headers: [
                            (
                                "content-length",
                                "0",
                            ),
                            (
                                "authorization",
                                "REDACTED",
                            ),
                        ],
                        body: None,
                    }
                    "#);
                }),
                include_bytes!("mock/team_members.json"),
                StatusCode::OK,
            ),
            (
                Box::new(|req| {
                    insta::assert_debug_snapshot!(req, @r#"
                    DebugReq {
                        method: GET,
                        uri: "/users/troykomodo",
                        headers: [
                            (
                                "content-length",
                                "0",
                            ),
                            (
                                "authorization",
                                "REDACTED",
                            ),
                        ],
                        body: None,
                    }
                    "#);
                }),
                include_bytes!("mock/user.json"),
                StatusCode::OK,
            ),
            (
                Box::new(|req| {
                    insta::assert_debug_snapshot!(req, @r#"
                    DebugReq {
                        method: GET,
                        uri: "/orgs/scufflecloud/teams/not-exist/members?per_page=100",
                        headers: [
                            (
                                "content-length",
                                "0",
                            ),
                            (
                                "authorization",
                                "REDACTED",
                            ),
                        ],
                        body: None,
                    }
                    "#);
                }),
                include_bytes!("mock/team_not_found.json"),
                StatusCode::NOT_FOUND,
            ),
            (
                Box::new(|req| {
                    insta::assert_debug_snapshot!(req, @r#"
                    DebugReq {
                        method: GET,
                        uri: "/users/not-exist",
                        headers: [
                            (
                                "content-length",
                                "0",
                            ),
                            (
                                "authorization",
                                "REDACTED",
                            ),
                        ],
                        body: None,
                    }
                    "#);
                }),
                include_bytes!("mock/user_not_found.json"),
                StatusCode::NOT_FOUND,
            ),
        ];

        for (case, resp_body, status_code) in cases {
            let (req, resp) = handle.next_request().await.unwrap();
            case(debug_req(req).await);
            resp.send_response(mock_response(*status_code, resp_body));
        }

        task.await.unwrap();
    }

    #[tokio::test]
    async fn test_repo_client_get_reviewers() {
        let (octocrab, mut handle) = mock_octocrab();
        let repo_client = mock_repo_client(
            octocrab,
            default_repo(),
            MockMergeWorkflow(1),
            Some("base".to_owned()),
        )
        .await;

        let task = tokio::spawn(async move {
            repo_client.get_reviewers(22).await.unwrap();
            repo_client
        });

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: GET,
            uri: "/repos/ScuffleCloud/ci-testing/pulls/22/reviews?per_page=100",
            headers: [
                (
                    "content-length",
                    "0",
                ),
                (
                    "authorization",
                    "REDACTED",
                ),
            ],
            body: None,
        }
        "#);

        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/pr_reviewers.json")));

        task.await.unwrap();
    }

    #[tokio::test]
    async fn test_repo_client_push_branch() {
        let (octocrab, mut handle) = mock_octocrab();
        let repo_client = mock_repo_client(
            octocrab,
            default_repo(),
            MockMergeWorkflow(1),
            Some("base".to_owned()),
        )
        .await;

        let task = tokio::spawn(async move {
            repo_client
                .push_branch("test/queue", "b7f8cd1bd474d5be1802377c9a0baea5eb59fcb7", true)
                .await
                .unwrap();
            repo_client
                .push_branch("test/queue", "b7f8cd1bd474d5be1802377c9a0baea5eb59fcb6", true)
                .await
                .unwrap();
            repo_client
                .push_branch("non-exist", "b7f8cd1bd474d5be1802377c9a0baea5eb59fcb6", true)
                .await
                .unwrap();
            repo_client
        });

        let (req, resp) = handle.next_request().await.unwrap();
        // Get latest ref commit for test/queue (sha is the same so no push) (case 1)
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: GET,
            uri: "/repositories/899726767/git/ref/heads/test/queue",
            headers: [
                (
                    "content-length",
                    "0",
                ),
                (
                    "authorization",
                    "REDACTED",
                ),
            ],
            body: None,
        }
        "#);
        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/ref_latest_commit.json")));

        // Get latest ref commit for test/queue (sha is different so push) (case 2)
        let (_, resp) = handle.next_request().await.unwrap();
        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/ref_latest_commit.json")));

        // update ref (case 2)
        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: PATCH,
            uri: "/repos/ScuffleCloud/ci-testing/git/refs/heads/test/queue",
            headers: [
                (
                    "content-type",
                    "application/json",
                ),
                (
                    "authorization",
                    "REDACTED",
                ),
            ],
            body: Some(
                Object {
                    "force": Bool(true),
                    "sha": String("b7f8cd1bd474d5be1802377c9a0baea5eb59fcb6"),
                },
            ),
        }
        "#);
        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/update_ref.json")));

        // get ref for non-exist (case 3)
        let (_, resp) = handle.next_request().await.unwrap();
        resp.send_response(mock_response(
            StatusCode::NOT_FOUND,
            include_bytes!("mock/ref_not_found.json"),
        ));

        // create ref (case 3)
        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: POST,
            uri: "/repositories/899726767/git/refs",
            headers: [
                (
                    "content-type",
                    "application/json",
                ),
                (
                    "authorization",
                    "REDACTED",
                ),
            ],
            body: Some(
                Object {
                    "ref": String("refs/heads/non-exist"),
                    "sha": String("b7f8cd1bd474d5be1802377c9a0baea5eb59fcb6"),
                },
            ),
        }
        "#);
        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/update_ref.json")));

        task.await.unwrap();
    }

    #[tokio::test]
    async fn test_repo_client_create_merge() {
        let (octocrab, mut handle) = mock_octocrab();
        let repo_client = mock_repo_client(
            octocrab,
            default_repo(),
            MockMergeWorkflow(1),
            Some("base".to_owned()),
        )
        .await;

        let task = tokio::spawn(async move {
            assert!(matches!(
                repo_client
                    .create_merge(
                        &messages::commit_message(
                            "https://github.com/ScuffleCloud/ci-testing/issues/22",
                            "test/queue",
                            "TroyKomodo,JohnDoe",
                            "TEST PR",
                            "some test pr body",
                            "TroyKomodo",
                        ),
                        "b7f8cd1bd474d5be1802377c9a0baea5eb59fcb7",
                        "b7f8cd1bd474d5be1802377c9a0baea5eb59fcb6",
                        &GitHubBrawlRepoConfig::default()
                    )
                    .await
                    .unwrap(),
                MergeResult::Success(_)
            ));
            assert!(matches!(
                repo_client
                    .create_merge(
                        &messages::commit_message(
                            "https://github.com/ScuffleCloud/ci-testing/issues/23",
                            "test/queue-conflict",
                            "TroyKomodo,JohnDoe",
                            "conflicting-pr",
                            "some conflicting pr body",
                            "TroyKomodo",
                        ),
                        "b7f8cd1bd474d5be1802377c9a0baea5eb59fcb7",
                        "conflicting-sha",
                        &GitHubBrawlRepoConfig::default()
                    )
                    .await
                    .unwrap(),
                MergeResult::Conflict
            ));
            repo_client
        });

        let mut settings = insta::Settings::new();
        settings.add_filter("[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}", "[UUID]");
        let _scope = settings.bind_to_scope();

        {
            // Push to tmp branch (case 1) (get ref latest commit)
            let (req, resp) = handle.next_request().await.unwrap();
            assert!(req.uri().path().starts_with("/repositories/899726767/git/ref/heads/"));
            assert_eq!(req.method(), Method::GET);
            resp.send_response(mock_response(
                StatusCode::NOT_FOUND,
                include_bytes!("mock/ref_not_found.json"),
            ));

            // create ref (case 1)
            let (req, resp) = handle.next_request().await.unwrap();
            assert!(req.uri().path().starts_with("/repositories/899726767/git/refs"));
            assert_eq!(req.method(), Method::POST);
            resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/update_ref.json")));

            // Create merge (case 1)
            let (req, resp) = handle.next_request().await.unwrap();

            insta::assert_debug_snapshot!(debug_req(req).await, @r#"
            DebugReq {
                method: POST,
                uri: "/repos/ScuffleCloud/ci-testing/merges",
                headers: [
                    (
                        "content-type",
                        "application/json",
                    ),
                    (
                        "authorization",
                        "REDACTED",
                    ),
                ],
                body: Some(
                    Object {
                        "base": String("automation/brawl/temp/[UUID]"),
                        "commit_message": String("Auto merge of https://github.com/ScuffleCloud/ci-testing/issues/22 - test/queue, r=TroyKomodo,JohnDoe\n\nTEST PR\nsome test pr body\n\nTroyKomodo\n"),
                        "head": String("b7f8cd1bd474d5be1802377c9a0baea5eb59fcb6"),
                    },
                ),
            }
            "#);
            resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/create_merge.json")));

            // Delete tmp branch (case 1)
            let (req, resp) = handle.next_request().await.unwrap();
            assert!(req.uri().path().starts_with("/repositories/899726767/git/refs/heads/"));
            assert_eq!(req.method(), Method::DELETE);
            resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/delete_branch.json")));
        }

        {
            // Push to tmp branch (case 2) (get ref latest commit)
            let (req, resp) = handle.next_request().await.unwrap();
            assert!(req.uri().path().starts_with("/repositories/899726767/git/ref/heads/"));
            assert_eq!(req.method(), Method::GET);
            resp.send_response(mock_response(
                StatusCode::NOT_FOUND,
                include_bytes!("mock/ref_not_found.json"),
            ));

            // create ref (case 2)
            let (req, resp) = handle.next_request().await.unwrap();
            assert!(req.uri().path().starts_with("/repositories/899726767/git/refs"));
            assert_eq!(req.method(), Method::POST);
            resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/update_ref.json")));

            // Create merge (case 2)
            let (req, resp) = handle.next_request().await.unwrap();
            insta::assert_debug_snapshot!(debug_req(req).await, @r#"
            DebugReq {
                method: POST,
                uri: "/repos/ScuffleCloud/ci-testing/merges",
                headers: [
                    (
                        "content-type",
                        "application/json",
                    ),
                    (
                        "authorization",
                        "REDACTED",
                    ),
                ],
                body: Some(
                    Object {
                        "base": String("automation/brawl/temp/[UUID]"),
                        "commit_message": String("Auto merge of https://github.com/ScuffleCloud/ci-testing/issues/23 - test/queue-conflict, r=TroyKomodo,JohnDoe\n\nconflicting-pr\nsome conflicting pr body\n\nTroyKomodo\n"),
                        "head": String("conflicting-sha"),
                    },
                ),
            }
            "#);
            resp.send_response(mock_response(
                StatusCode::CONFLICT,
                include_bytes!("mock/create_merge_conflict.json"),
            ));

            // Delete tmp branch (case 2)
            let (req, resp) = handle.next_request().await.unwrap();
            assert!(req.uri().path().starts_with("/repositories/899726767/git/refs/heads/"));
            assert_eq!(req.method(), Method::DELETE);
            resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/delete_branch.json")));
        }

        task.await.unwrap();
    }

    #[tokio::test]
    async fn test_repo_client_add_labels() {
        let (octocrab, mut handle) = mock_octocrab();
        let repo_client = mock_repo_client(
            octocrab,
            default_repo(),
            MockMergeWorkflow(1),
            Some("base".to_owned()),
        )
        .await;

        let task = tokio::spawn(async move {
            repo_client.add_labels(1, &["queued".to_string()]).await.unwrap();
            repo_client
        });

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: POST,
            uri: "/repositories/899726767/issues/1/labels",
            headers: [
                (
                    "content-type",
                    "application/json",
                ),
                (
                    "authorization",
                    "REDACTED",
                ),
            ],
            body: Some(
                Object {
                    "labels": Array [
                        String("queued"),
                    ],
                },
            ),
        }
        "#);

        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/add_labels.json")));

        task.await.unwrap();
    }

    #[tokio::test]
    async fn test_repo_client_remove_labels() {
        let (octocrab, mut handle) = mock_octocrab();
        let repo_client = mock_repo_client(
            octocrab,
            default_repo(),
            MockMergeWorkflow(1),
            Some("base".to_owned()),
        )
        .await;

        let task = tokio::spawn(async move {
            repo_client.remove_label(1, "some_label").await.unwrap();
            repo_client
        });

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: DELETE,
            uri: "/repositories/899726767/issues/1/labels/some%5Flabel",
            headers: [
                (
                    "content-length",
                    "0",
                ),
                (
                    "authorization",
                    "REDACTED",
                ),
            ],
            body: None,
        }
        "#);

        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/remove_label.json")));

        task.await.unwrap();
    }

    #[tokio::test]
    async fn test_repo_client_workflow_run_link() {
        let octocrab = octocrab::Octocrab::builder().personal_token("token").build().unwrap();
        let repo = mock_repo_client(
            octocrab,
            default_repo(),
            MockMergeWorkflow(1),
            Some("base".to_owned()),
        )
        .await;
        assert_eq!(
            repo.workflow_run_link(RunId(1)),
            "https://github.com/ScuffleCloud/ci-testing/actions/runs/1"
        );
    }

    #[tokio::test]
    async fn test_repo_client_pr_link() {
        let octocrab = octocrab::Octocrab::builder().personal_token("token").build().unwrap();
        let repo = mock_repo_client(
            octocrab,
            default_repo(),
            MockMergeWorkflow(1),
            Some("base".to_owned()),
        )
        .await;
        assert_eq!(repo.pr_link(1), "https://github.com/ScuffleCloud/ci-testing/pull/1");
    }

    #[tokio::test]
    async fn test_repo_client_owner() {
        let octocrab = octocrab::Octocrab::builder().personal_token("token").build().unwrap();
        let repo = mock_repo_client(
            octocrab,
            default_repo(),
            MockMergeWorkflow(1),
            Some("base".to_owned()),
        )
        .await;
        assert_eq!(repo.owner(), "ScuffleCloud");
    }

    #[tokio::test]
    async fn test_repo_client_name() {
        let octocrab = octocrab::Octocrab::builder().personal_token("token").build().unwrap();
        let repo = mock_repo_client(
            octocrab,
            default_repo(),
            MockMergeWorkflow(1),
            Some("base".to_owned()),
        )
        .await;
        assert_eq!(repo.name(), "ci-testing");
    }

    #[tokio::test]
    async fn test_repo_client_commit_link() {
        let octocrab = octocrab::Octocrab::builder().personal_token("token").build().unwrap();
        let repo = mock_repo_client(
            octocrab,
            default_repo(),
            MockMergeWorkflow(1),
            Some("base".to_owned()),
        )
        .await;
        assert_eq!(
            repo.commit_link("b7f8cd1bd474d5be1802377c9a0baea5eb59fcb6"),
            "https://github.com/ScuffleCloud/ci-testing/commit/b7f8cd1bd474d5be1802377c9a0baea5eb59fcb6"
        );
    }

    #[tokio::test]
    async fn test_repo_client_get_workflow_runs() {
        let (octocrab, mut handle) = mock_octocrab();

        let repo = mock_repo_client(
            octocrab,
            default_repo(),
            MockMergeWorkflow(1),
            Some("base".to_owned()),
        )
        .await;
        let task = tokio::spawn(async move {
            repo.branch_workflows("main").await.unwrap();
        });

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: GET,
            uri: "/repos/ScuffleCloud/ci-testing/actions/runs?branch=main&per_page=100&page=1",
            headers: [
                (
                    "content-length",
                    "0",
                ),
                (
                    "authorization",
                    "REDACTED",
                ),
            ],
            body: None,
        }
        "#);
        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/get_workflow_runs.json")));

        task.await.unwrap();
    }

    #[tokio::test]
    async fn test_repo_client_cancel_workflow_run() {
        let (octocrab, mut handle) = mock_octocrab();
        let repo = mock_repo_client(
            octocrab,
            default_repo(),
            MockMergeWorkflow(1),
            Some("base".to_owned()),
        )
        .await;
        let task = tokio::spawn(async move {
            repo.cancel_workflow_run(RunId(1)).await.unwrap();
        });

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: POST,
            uri: "/repos/ScuffleCloud/ci-testing/actions/runs/1/cancel",
            headers: [
                (
                    "content-length",
                    "0",
                ),
                (
                    "authorization",
                    "REDACTED",
                ),
            ],
            body: None,
        }
        "#);

        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/cancel_workflow_run.json")));

        task.await.unwrap();
    }

    #[tokio::test]
    async fn test_repo_get_config() {
        let (octocrab, mut handle) = mock_octocrab();
        let repo = mock_repo_client(octocrab, default_repo(), MockMergeWorkflow(1), Some("base".to_owned())).await;

        let task = tokio::spawn(async move {
            let config = repo.config().await.unwrap().unwrap();
            assert_eq!(config.enabled, true);
        });

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: GET,
            uri: "/repositories/899726767/contents/.github/brawl.toml?ref=base",
            headers: [
                (
                    "content-length",
                    "0",
                ),
                (
                    "authorization",
                    "REDACTED",
                ),
            ],
            body: None,
        }
        "#);

        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/get_config.json")));

        task.await.unwrap();
    }

    #[tokio::test]
    async fn test_repo_get_config_at_ref() {
        let (octocrab, mut handle) = mock_octocrab();
        let repo = mock_repo_client(octocrab, default_repo(), MockMergeWorkflow(1), Some("base".to_owned())).await;

        let task = tokio::spawn(async move {
            let config = repo.config_at_commit("b7f8cd1bd474d5be1802377c9a0baea5eb59fcb6").await.unwrap().unwrap();
            assert_eq!(config.enabled, true);
        });

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: GET,
            uri: "/repositories/899726767/contents/.github/brawl.toml?ref=b7f8cd1bd474d5be1802377c9a0baea5eb59fcb6",
            headers: [
                (
                    "content-length",
                    "0",
                ),
                (
                    "authorization",
                    "REDACTED",
                ),
            ],
            body: None,
        }
        "#);

        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/get_config.json")));

        task.await.unwrap();
    }
}
