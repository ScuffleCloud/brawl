use chrono::{DateTime, Utc};
use octocrab::models::pulls::{MergeableState, ReviewState};
use octocrab::models::{InstallationId, IssueState, RepositoryId, RunId, UserId, WorkflowId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub login: String,
    pub id: UserId,
}

impl Default for User {
    fn default() -> Self {
        Self {
            login: "user".to_string(),
            id: UserId(0),
        }
    }
}

impl From<octocrab::models::UserProfile> for User {
    fn from(value: octocrab::models::UserProfile) -> Self {
        Self {
            login: value.login,
            id: value.id,
        }
    }
}

impl From<octocrab::models::Author> for User {
    fn from(value: octocrab::models::Author) -> Self {
        Self {
            login: value.login,
            id: value.id,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Label {
    pub name: String,
}

impl From<octocrab::models::Label> for Label {
    fn from(value: octocrab::models::Label) -> Self {
        Self { name: value.name }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PrBranch {
    pub label: Option<String>,
    pub ref_field: String,
    pub sha: String,
    pub user: Option<User>,
    pub repo: Option<Repository>,
}

impl From<octocrab::models::pulls::Head> for PrBranch {
    fn from(value: octocrab::models::pulls::Head) -> Self {
        Self {
            label: value.label,
            ref_field: value.ref_field,
            sha: value.sha,
            user: value.user.map(|u| u.into()),
            repo: value.repo.map(|r| r.into()),
        }
    }
}

impl From<octocrab::models::pulls::Base> for PrBranch {
    fn from(value: octocrab::models::pulls::Base) -> Self {
        Self {
            label: value.label,
            ref_field: value.ref_field,
            sha: value.sha,
            user: value.user.map(|u| u.into()),
            repo: value.repo.map(|r| r.into()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub body: String,
    pub state: Option<IssueState>,
    pub mergeable_state: Option<MergeableState>,
    pub merge_commit_sha: Option<String>,
    pub assignees: Vec<User>,
    pub requested_reviewers: Vec<User>,
    pub user: Option<User>,
    pub labels: Vec<Label>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub draft: Option<bool>,
    pub closed_at: Option<DateTime<Utc>>,
    pub merged_at: Option<DateTime<Utc>>,
    pub head: PrBranch,
    pub base: PrBranch,
}

impl From<octocrab::models::pulls::PullRequest> for PullRequest {
    fn from(value: octocrab::models::pulls::PullRequest) -> Self {
        Self {
            number: value.number,
            title: value.title.unwrap_or_default(),
            body: value.body.unwrap_or_default(),
            state: value.state,
            mergeable_state: value.mergeable_state,
            merge_commit_sha: value.merge_commit_sha,
            assignees: value.assignees.into_iter().flatten().map(|a| a.into()).collect(),
            requested_reviewers: value.requested_reviewers.into_iter().flatten().map(|a| a.into()).collect(),
            user: value.user.map(|u| (*u).into()),
            labels: value.labels.into_iter().flatten().map(|l| l.into()).collect(),
            created_at: value.created_at,
            updated_at: value.updated_at,
            draft: value.draft,
            closed_at: value.closed_at,
            merged_at: value.merged_at,
            head: (*value.head).into(),
            base: (*value.base).into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Review {
    pub user: Option<User>,
    pub state: Option<ReviewState>,
}

impl From<octocrab::models::pulls::Review> for Review {
    fn from(value: octocrab::models::pulls::Review) -> Self {
        Self {
            user: value.user.map(|u| u.into()),
            state: value.state,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Commit {
    pub sha: String,
    pub tree: Option<String>,
}

impl From<octocrab::models::commits::Commit> for Commit {
    fn from(value: octocrab::models::commits::Commit) -> Self {
        Self {
            sha: value.sha,
            tree: Some(value.commit.tree.sha),
        }
    }
}

impl From<octocrab::models::commits::GitCommitObject> for Commit {
    fn from(value: octocrab::models::commits::GitCommitObject) -> Self {
        Self {
            sha: value.sha,
            tree: Some(value.tree.sha),
        }
    }
}

#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CheckRunStatus {
    #[default]
    Queued,
    InProgress,
    Completed,
    Waiting,
    Requested,
    Pending,
}

#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CheckRunConclusion {
    Success,
    Failure,
    #[default]
    Neutral,
    Cancelled,
    Skipped,
    TimedOut,
    ActionRequired,
}

#[derive(Debug, Clone, serde::Deserialize, PartialEq, Eq, Default)]
pub struct CheckRunEvent {
    pub id: i64,
    pub name: String,
    pub head_sha: String,
    pub html_url: Option<String>,
    pub details_url: Option<String>,
    pub url: String,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub status: CheckRunStatus,
    pub conclusion: Option<CheckRunConclusion>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Repository {
    pub id: RepositoryId,
    pub name: String,
    pub owner: User,
    pub default_branch: Option<String>,
}

impl From<octocrab::models::Repository> for Repository {
    fn from(value: octocrab::models::Repository) -> Self {
        Self {
            id: value.id,
            name: value.name,
            owner: value.owner.expect("repository has no owner").into(),
            default_branch: value.default_branch,
        }
    }
}

impl Default for Repository {
    fn default() -> Self {
        Self {
            id: RepositoryId(0),
            name: "repo".to_string(),
            owner: User::default(),
            default_branch: Some("main".to_string()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installation {
    pub id: InstallationId,
    pub account: User,
}

impl Default for Installation {
    fn default() -> Self {
        Self {
            id: InstallationId(0),
            account: User::default(),
        }
    }
}

impl From<octocrab::models::Installation> for Installation {
    fn from(value: octocrab::models::Installation) -> Self {
        Self {
            id: value.id,
            account: value.account.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkflowRun {
    pub id: RunId,
    pub workflow_id: WorkflowId,
    pub name: String,
    pub head_sha: String,
    pub head_branch: String,
    pub conclusion: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub status: String,
}

impl From<octocrab::models::workflows::Run> for WorkflowRun {
    fn from(value: octocrab::models::workflows::Run) -> Self {
        Self {
            id: value.id,
            workflow_id: value.workflow_id,
            name: value.name,
            head_sha: value.head_sha,
            head_branch: value.head_branch,
            conclusion: value.conclusion,
            created_at: value.created_at,
            updated_at: value.updated_at,
            status: value.status,
        }
    }
}

impl Default for WorkflowRun {
    fn default() -> Self {
        Self {
            id: RunId(0),
            workflow_id: WorkflowId(0),
            name: "workflow".to_string(),
            head_sha: "sha".to_string(),
            head_branch: "branch".to_string(),
            conclusion: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            status: "status".to_string(),
        }
    }
}
