use std::fmt::Display;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

#[derive(Debug, Deserialize, Serialize, Clone, smart_default::SmartDefault)]
#[serde(default)]
pub struct GitHubBrawlRepoConfig {
    /// Whether Brawl is enabled for this repo
    #[default(true)]
    pub enabled: bool,
    /// Labels to attach to PRs on different states
    pub labels: GitHubBrawlLabelsConfig,
    /// The target branches that this queue matches against.
    pub branches: Vec<String>,
    /// The branch prefix for @brawl try commands
    /// (default: "automation/brawl/try/")
    #[default("automation/brawl/try/")]
    pub try_branch_prefix: String,
    /// The branch prefix for @brawl merge commands
    /// (default: "automation/brawl/merge/")
    #[default("automation/brawl/merge/")]
    pub merge_branch_prefix: String,
    /// The branch prefix for temp branches used when performing merges
    /// (default: "automation/brawl/temp/")
    #[default("automation/brawl/temp/")]
    pub temp_branch_prefix: String,
    /// The permissions required to merge a PR
    /// (default: ["role:write"])
    #[default(vec![Permission::Role(Role::Push)])]
    pub merge_permissions: Vec<Permission>,
    /// The status checks required to merge a PR
    /// (default: ["brawl-done"])
    ///
    /// If brawl will wait for all of these status checks to be successful
    /// before merging. If not provided the PR will be merged instantly.
    #[default(vec![
		"brawl-done".to_string()
	])]
    pub required_status_checks: Vec<String>,
    /// The number of minutes to wait before declaring the merge failed if the
    /// required status checks are not met.
    #[default(60)]
    pub timeout_minutes: i32,
    /// The permissions required to try a commit
    /// (default: <same as merge permissions>)
    #[default(None)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub try_permissions: Option<Vec<Permission>>,
    /// The maximum number of reviewers for a PR
    /// (default: 10)
    #[default(10)]
    pub max_reviewers: i32,
    /// The permissions required to be a reviewer
    /// (default: <same as merge permissions>)
    #[default(None)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reviewer_permissions: Option<Vec<Permission>>,
}

impl GitHubBrawlRepoConfig {
    pub fn missing() -> Self {
        Self {
            enabled: false,
            ..Default::default()
        }
    }

    pub fn try_permissions(&self) -> &[Permission] {
        self.try_permissions.as_ref().unwrap_or(&self.merge_permissions)
    }

    pub fn reviewer_permissions(&self) -> &[Permission] {
        self.reviewer_permissions.as_ref().unwrap_or(&self.merge_permissions)
    }

    pub fn merge_branch(&self, ref_field: &str) -> String {
        format!("{}/{}", self.merge_branch_prefix.trim_end_matches('/'), ref_field)
    }

    pub fn temp_branch(&self) -> String {
        format!("{}/{}", self.temp_branch_prefix.trim_end_matches('/'), uuid::Uuid::new_v4())
    }

    pub fn try_branch(&self, pr_number: u64) -> String {
        format!("{}/{}", self.try_branch_prefix.trim_end_matches('/'), pr_number)
    }
}

#[derive(Debug, Deserialize, Serialize, Clone, smart_default::SmartDefault)]
#[serde(default)]
pub struct GitHubBrawlLabelsConfig {
    /// The label to attach to PRs when they are in the merge queue
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "string_or_vec")]
    pub on_merge_queued: Vec<String>,
    /// The label to attach to PRs when they are being merged
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "string_or_vec")]
    pub on_merge_in_progress: Vec<String>,
    /// The label to attach to PRs when they fail to merge
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "string_or_vec")]
    pub on_merge_failure: Vec<String>,
    /// The label to attach to PRs when they are merged
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "string_or_vec")]
    pub on_merge_success: Vec<String>,
    /// The label to attach to PRs when they are being tried
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "string_or_vec")]
    pub on_try_in_progress: Vec<String>,
    /// The label to attach to PRs when they fail to try
    #[serde(skip_serializing_if = "Vec::is_empty", deserialize_with = "string_or_vec")]
    pub on_try_failure: Vec<String>,
}

fn string_or_vec<'de, D: Deserializer<'de>>(s: D) -> Result<Vec<String>, D::Error> {
    use serde::de::SeqAccess;

    struct StringOrVecVisitor;

    impl<'de> serde::de::Visitor<'de> for StringOrVecVisitor {
        type Value = Vec<String>;

        #[cfg_attr(coverage_nightly, coverage(off))]
        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a string or a vector of strings")
        }

        fn visit_str<E>(self, v: &str) -> Result<Self::Value, E> {
            Ok(vec![v.to_string()])
        }

        fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut vec = Vec::new();
            while let Some(value) = seq.next_element()? {
                vec.push(value);
            }
            Ok(vec)
        }
    }

    s.deserialize_any(StringOrVecVisitor)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Permission {
    Role(Role),
    Team(String),
    User(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Role {
    Pull,
    Push,
    Admin,
    Maintain,
    Triage,
}

impl From<Role> for octocrab::params::teams::Permission {
    fn from(role: Role) -> Self {
        match role {
            Role::Pull => Self::Pull,
            Role::Push => Self::Push,
            Role::Admin => Self::Admin,
            Role::Maintain => Self::Maintain,
            Role::Triage => Self::Triage,
        }
    }
}

impl FromStr for Role {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "pull" | "read" => Self::Pull,
            "push" | "write" => Self::Push,
            "triage" => Self::Triage,
            "maintain" => Self::Maintain,
            "admin" => Self::Admin,
            _ => return Err(()),
        })
    }
}

impl Display for Role {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Role::Pull => write!(f, "pull"),
            Role::Push => write!(f, "push"),
            Role::Admin => write!(f, "admin"),
            Role::Maintain => write!(f, "maintain"),
            Role::Triage => write!(f, "triage"),
        }
    }
}

impl FromStr for Permission {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (prefix, value) = s.split_once(':').ok_or(())?;

        Ok(match prefix {
            "role" => Self::Role(value.parse().map_err(|_| ())?),
            "team" => Self::Team(value.to_string()),
            "user" => Self::User(value.to_string()),
            _ => return Err(()),
        })
    }
}

impl Display for Permission {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Permission::Role(role) => write!(f, "role:{}", role),
            Permission::Team(team) => write!(f, "team:{}", team),
            Permission::User(user) => write!(f, "user:{}", user),
        }
    }
}

impl<'de> Deserialize<'de> for Permission {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        s.parse().map_err(|_| serde::de::Error::custom("invalid permission"))
    }
}

impl Serialize for Permission {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_from_str() {
        let cases = [
            ("role:pull", Ok(Permission::Role(Role::Pull))),
            ("role:push", Ok(Permission::Role(Role::Push))),
            ("role:read", Ok(Permission::Role(Role::Pull))),
            ("role:write", Ok(Permission::Role(Role::Push))),
            ("role:admin", Ok(Permission::Role(Role::Admin))),
            ("role:maintain", Ok(Permission::Role(Role::Maintain))),
            ("role:triage", Ok(Permission::Role(Role::Triage))),
            ("role:unknown", Err(())),
            ("team:brawl", Ok(Permission::Team("brawl".to_string()))),
            ("user:brawl", Ok(Permission::User("brawl".to_string()))),
            ("team:brawl:brawl", Ok(Permission::Team("brawl:brawl".to_string()))),
            ("user:brawl:brawl", Ok(Permission::User("brawl:brawl".to_string()))),
            ("weird:brawl", Err(())),
            ("invalid", Err(())),
        ];

        for (input, expected) in cases {
            assert_eq!(input.parse::<Permission>(), expected);
        }
    }

    #[test]
    fn test_deserialize() {
        let cases = [
            ("\"role:pull\"", Ok(Permission::Role(Role::Pull))),
            ("\"team:brawl\"", Ok(Permission::Team("brawl".to_string()))),
            ("\"user:brawl\"", Ok(Permission::User("brawl".to_string()))),
            ("\"invalid\"", Err(())),
        ];

        for (input, expected) in cases {
            let result = serde_json::from_str::<Permission>(input);
            match (result, expected) {
                (Ok(result), Ok(expected)) => assert_eq!(result, expected),
                (Err(e), Ok(_)) => panic!("expected ok, got error: {e}"),
                (Ok(_), Err(_)) => panic!("expected error"),
                (Err(_), Err(_)) => {}
            }
        }
    }

    #[test]
    fn test_role_convert() {
        matches!(
            octocrab::params::teams::Permission::from(Role::Pull),
            octocrab::params::teams::Permission::Pull
        );
        matches!(
            octocrab::params::teams::Permission::from(Role::Push),
            octocrab::params::teams::Permission::Push
        );
        matches!(
            octocrab::params::teams::Permission::from(Role::Admin),
            octocrab::params::teams::Permission::Admin
        );
        matches!(
            octocrab::params::teams::Permission::from(Role::Maintain),
            octocrab::params::teams::Permission::Maintain
        );
        matches!(
            octocrab::params::teams::Permission::from(Role::Triage),
            octocrab::params::teams::Permission::Triage
        );
    }

    #[test]
    fn test_missing_disabled() {
        let config = GitHubBrawlRepoConfig::missing();
        assert!(!config.enabled);
    }

    #[test]
    fn test_default_config() {
        let config = GitHubBrawlRepoConfig::default();
        let s = toml::to_string_pretty(&config).unwrap();
        insta::assert_snapshot!(s, @r#"
        enabled = true
        branches = []
        try_branch_prefix = "automation/brawl/try/"
        merge_branch_prefix = "automation/brawl/merge/"
        temp_branch_prefix = "automation/brawl/temp/"
        merge_permissions = ["role:push"]
        required_status_checks = ["brawl-done"]
        timeout_minutes = 60
        max_reviewers = 10

        [labels]
        "#);
    }

    #[test]
    fn test_config_deserialize() {
        let config = r#"
        enabled = true
        branches = [
            "main",
            "staging",
        ]
        try_branch_prefix = "automation/brawl/try/"
        merge_branch_prefix = "automation/brawl/merge/"
        temp_branch_prefix = "automation/brawl/temp/"
        merge_permissions = ["role:push"]
        required_status_checks = ["brawl-done"]
        timeout_minutes = 60
        max_reviewers = 10

        [labels]
        on_merge_queued = "merge-queued"
        on_merge_in_progress = ["merge-in-progress", "waiting-on-brawl"]
        on_merge_failure = "merge-failed"
        on_merge_success = "merge-success"
        on_try_in_progress = ["try-in-progress", "waiting-on-brawl"]
        on_try_failure = "try-failed"
        "#;

        let config: GitHubBrawlRepoConfig = toml::from_str(config).unwrap();
        assert!(config.enabled);
        assert_eq!(config.branches, vec!["main", "staging"]);
        assert_eq!(config.try_branch_prefix, "automation/brawl/try/");
        assert_eq!(config.merge_branch_prefix, "automation/brawl/merge/");
        assert_eq!(config.temp_branch_prefix, "automation/brawl/temp/");
        assert_eq!(config.merge_permissions, vec![Permission::Role(Role::Push)]);
        assert_eq!(config.required_status_checks, vec!["brawl-done"]);
        assert_eq!(config.timeout_minutes, 60);
        assert_eq!(config.max_reviewers, 10);
        assert_eq!(config.labels.on_merge_queued, vec!["merge-queued".to_string()]);
        assert_eq!(
            config.labels.on_merge_in_progress,
            vec!["merge-in-progress".to_string(), "waiting-on-brawl".to_string()]
        );
        assert_eq!(config.labels.on_merge_failure, vec!["merge-failed".to_string()]);
        assert_eq!(config.labels.on_merge_success, vec!["merge-success".to_string()]);
        assert_eq!(
            config.labels.on_try_in_progress,
            vec!["try-in-progress".to_string(), "waiting-on-brawl".to_string()]
        );
        assert_eq!(config.labels.on_try_failure, vec!["try-failed".to_string()]);
    }

    #[test]
    fn test_role_to_string() {
        assert_eq!(Role::Pull.to_string(), "pull");
        assert_eq!(Role::Push.to_string(), "push");
        assert_eq!(Role::Admin.to_string(), "admin");
        assert_eq!(Role::Maintain.to_string(), "maintain");
        assert_eq!(Role::Triage.to_string(), "triage");
    }

    #[test]
    fn test_permission_to_string() {
        assert_eq!(Permission::Role(Role::Pull).to_string(), "role:pull");
        assert_eq!(Permission::Team("brawl".to_string()).to_string(), "team:brawl");
        assert_eq!(Permission::User("brawl".to_string()).to_string(), "user:brawl");
    }

    #[test]
    fn test_try_permissions() {
        let config = GitHubBrawlRepoConfig::default();
        assert_eq!(config.try_permissions(), &config.merge_permissions);
        let config = GitHubBrawlRepoConfig {
            try_permissions: Some(vec![Permission::Role(Role::Push)]),
            ..Default::default()
        };
        assert_eq!(config.try_permissions(), config.try_permissions.as_deref().unwrap());
    }

    #[test]
    fn test_reviewer_permissions() {
        let config = GitHubBrawlRepoConfig::default();
        assert_eq!(config.reviewer_permissions(), &config.merge_permissions);
        let config = GitHubBrawlRepoConfig {
            reviewer_permissions: Some(vec![Permission::Role(Role::Push)]),
            ..Default::default()
        };
        assert_eq!(config.reviewer_permissions(), config.reviewer_permissions.as_deref().unwrap());
    }

    #[test]
    fn test_merge_branch() {
        let config = GitHubBrawlRepoConfig::default();
        assert_eq!(config.merge_branch("main"), "automation/brawl/merge/main");
        let config = GitHubBrawlRepoConfig {
            merge_branch_prefix: "automation/brawl/merge-2a".to_string(),
            ..Default::default()
        };
        assert_eq!(config.merge_branch("main"), "automation/brawl/merge-2a/main");
        let config = GitHubBrawlRepoConfig {
            merge_branch_prefix: "automation/brawl/merge-3a///////////".to_string(),
            ..Default::default()
        };
        assert_eq!(config.merge_branch("main"), "automation/brawl/merge-3a/main");
    }

    #[test]
    fn test_temp_branch() {
        let config = GitHubBrawlRepoConfig::default();
        assert!(config.temp_branch().starts_with("automation/brawl/temp/"));
        let config = GitHubBrawlRepoConfig {
            temp_branch_prefix: "automation/brawl/temp-2a".to_string(),
            ..Default::default()
        };
        assert!(config.temp_branch().starts_with("automation/brawl/temp-2a/"));
        let config = GitHubBrawlRepoConfig {
            temp_branch_prefix: "automation/brawl/temp-3a///////////".to_string(),
            ..Default::default()
        };
        assert!(config.temp_branch().starts_with("automation/brawl/temp-3a/"));
    }

    #[test]
    fn test_try_branch() {
        let config = GitHubBrawlRepoConfig::default();
        assert_eq!(config.try_branch(123), "automation/brawl/try/123");
        let config = GitHubBrawlRepoConfig {
            try_branch_prefix: "automation/brawl/try-2a".to_string(),
            ..Default::default()
        };
        assert_eq!(config.try_branch(123), "automation/brawl/try-2a/123");
        let config = GitHubBrawlRepoConfig {
            try_branch_prefix: "automation/brawl/try-3a///////////".to_string(),
            ..Default::default()
        };
        assert_eq!(config.try_branch(123), "automation/brawl/try-3a/123");
    }
}
