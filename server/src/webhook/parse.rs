use std::str::FromStr;

use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode};
use hmac::{Hmac, Mac};
use octocrab::models::webhook_events::payload::{
    InstallationWebhookEventAction, IssueCommentWebhookEventAction, RepositoryWebhookEventAction,
};
use octocrab::models::webhook_events::{EventInstallation, WebhookEventPayload, WebhookEventType};
use octocrab::models::{InstallationId, RepositoryId};
use sha2::Sha256;

use crate::command::BrawlCommand;
use crate::github::models::{Installation, Repository, User};

fn verify_gh_signature(headers: &HeaderMap<HeaderValue>, body: &[u8], secret: &str) -> bool {
    let Some(signature) = headers.get("x-hub-signature-256").map(|v| v.as_bytes()) else {
        return false;
    };
    let Some(signature) = signature.get(b"sha256=".len()..).and_then(|v| hex::decode(v).ok()) else {
        return false;
    };

    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("Cannot create HMAC key");
    mac.update(body);
    mac.verify_slice(&signature).is_ok()
}

#[derive(Debug, Clone)]
struct WebhookEvent {
    pub sender: Option<User>,
    pub repository: Option<Repository>,
    pub installtion_id: Option<InstallationId>,
    pub installation: Option<Installation>,
    pub kind: WebhookEventType,
    pub specific: WebhookEventPayload,
}

#[derive(Debug, Clone)]
pub enum WebhookEventAction {
    Command {
        installation_id: InstallationId,
        command: BrawlCommand,
        repo_id: RepositoryId,
        pr_number: u64,
        user: User,
    },
    PullRequest {
        installation_id: InstallationId,
        repo_id: RepositoryId,
        pr_number: u64,
        user: User,
    },
    CheckRun {
        installation_id: InstallationId,
        repo_id: RepositoryId,
        check_run: serde_json::Value,
    },
    DeleteInstallation {
        installation_id: InstallationId,
    },
    AddRepository {
        installation_id: InstallationId,
        repo_id: RepositoryId,
    },
    RemoveRepository {
        installation_id: InstallationId,
        repo_id: RepositoryId,
    },
    UpdateInstallation {
        installation: Installation,
    },
}

fn parse_event(header: String, body: &[u8]) -> Result<Vec<WebhookEventAction>, (StatusCode, String)> {
    let kind: WebhookEventType =
        serde_json::from_value(serde_json::Value::String(header)).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // Intermediate structure allows to separate the common fields from
    // the event specific one.
    #[derive(serde::Deserialize)]
    struct Intermediate {
        sender: Option<octocrab::models::Author>,
        repository: Option<octocrab::models::Repository>,
        #[allow(unused)]
        organization: Option<octocrab::models::orgs::Organization>,
        installation: Option<octocrab::models::webhook_events::EventInstallation>,
        #[serde(flatten)]
        specific: serde_json::Value,
    }

    let Intermediate {
        sender,
        repository,
        organization: _,
        installation,
        mut specific,
    } = serde_json::from_slice::<Intermediate>(body).map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    // Bug: OctoCrab wrongly requires the pusher to have an email
    // Remove when https://github.com/XAMPPRocky/octocrab/issues/486 is fixed
    if kind == WebhookEventType::Push {
        if let Some(pusher) = specific.get_mut("pusher") {
            if let Some(email) = pusher.get_mut("email") {
                if email.is_null() {
                    *email = serde_json::Value::String("".to_owned())
                }
            }
        }
    }

    let specific = kind
        .parse_specific_payload(specific)
        .map_err(|e| (StatusCode::BAD_REQUEST, e.to_string()))?;

    let actions = parse_webhook(WebhookEvent {
        sender: sender.map(|s| s.into()),
        repository: repository.map(|r| r.into()),
        installtion_id: match &installation {
            Some(EventInstallation::Full(installation)) => Some(installation.id),
            Some(EventInstallation::Minimal(installation)) => Some(installation.id),
            None => None,
        },
        installation: match installation {
            Some(EventInstallation::Full(installation)) => Some((*installation).into()),
            _ => None,
        },
        kind,
        specific,
    });

    Ok(actions)
}

fn parse_webhook(mut event: WebhookEvent) -> Vec<WebhookEventAction> {
    let mut actions = vec![];

    if let Some(installation) = event.installation {
        actions.push(WebhookEventAction::UpdateInstallation { installation });
    }

    let Some(installation_id) = event.installtion_id else {
        tracing::warn!("event does not have an installation id: {:?}", event.kind);
        return actions;
    };

    match event.specific {
        WebhookEventPayload::Installation(install_event) => match install_event.action {
            InstallationWebhookEventAction::Deleted | InstallationWebhookEventAction::Suspend => {
                actions.push(WebhookEventAction::DeleteInstallation { installation_id });
            }
            _ => {}
        },
        WebhookEventPayload::InstallationRepositories(event) => {
            for repo in event.repositories_added {
                actions.push(WebhookEventAction::AddRepository {
                    installation_id,
                    repo_id: repo.id,
                });
            }

            for repo in event.repositories_removed {
                actions.push(WebhookEventAction::RemoveRepository {
                    installation_id,
                    repo_id: repo.id,
                });
            }
        }
        WebhookEventPayload::Repository(repo_event) => {
            let Some(repo) = event.repository else {
                return actions;
            };

            match repo_event.action {
                RepositoryWebhookEventAction::Deleted | RepositoryWebhookEventAction::Archived => {
                    actions.push(WebhookEventAction::RemoveRepository {
                        installation_id,
                        repo_id: repo.id,
                    });
                }
                _ => {
                    actions.push(WebhookEventAction::AddRepository {
                        installation_id,
                        repo_id: repo.id,
                    });
                }
            }
        }
        WebhookEventPayload::PullRequest(mut pull_request_event) => {
            let Some(repo_id) = event.repository.as_ref().map(|r| r.id).or(pull_request_event
                .pull_request
                .repo
                .as_deref()
                .map(|r| r.id))
            else {
                return actions;
            };

            let Some(user) = event
                .sender
                .take()
                .or_else(|| pull_request_event.pull_request.user.take().map(|u| (*u).into()))
            else {
                return actions;
            };

            actions.push(WebhookEventAction::PullRequest {
                installation_id,
                repo_id,
                pr_number: pull_request_event.pull_request.number,
                user,
            });
        }
        WebhookEventPayload::IssueComment(issue_comment_event)
            if issue_comment_event.action == IssueCommentWebhookEventAction::Created
                && issue_comment_event.issue.pull_request.is_some() =>
        {
            let Some(body) = issue_comment_event.comment.body.as_ref() else {
                return actions;
            };

            let Ok(command) = BrawlCommand::from_str(body) else {
                return actions;
            };

            let Some(repo) = event.repository else {
                return actions;
            };

            actions.push(WebhookEventAction::Command {
                installation_id,
                command,
                repo_id: repo.id,
                pr_number: issue_comment_event.issue.number,
                user: issue_comment_event.comment.user.into(),
            });
        }
        WebhookEventPayload::CheckRun(check_run_event) => {
            let Some(repo) = event.repository else {
                return actions;
            };

            actions.push(WebhookEventAction::CheckRun {
                installation_id,
                repo_id: repo.id,
                check_run: check_run_event.check_run,
            });
        }
        _ => {}
    }

    actions
}

pub async fn parse_from_request(
    request: Request<Body>,
    secret: &str,
) -> Result<Vec<WebhookEventAction>, (StatusCode, String)> {
    let (parts, body) = request.into_parts();
    let Some(header) = parts.headers.get("X-GitHub-Event").and_then(|v| v.to_str().ok()) else {
        return Err((StatusCode::BAD_REQUEST, "Missing X-GitHub-Event header".to_string()));
    };

    let Ok(body) = axum::body::to_bytes(body, 1024 * 1024 * 10).await else {
        return Err((StatusCode::BAD_REQUEST, "Failed to read body".to_string()));
    };

    if !verify_gh_signature(&parts.headers, &body, secret) {
        return Err((StatusCode::UNAUTHORIZED, "Invalid signature".to_string()));
    }

    parse_event(header.to_owned(), &body)
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn test_parse_event() {
        let event = parse_event("check_run".to_string(), include_bytes!("mock/webhook.check_run.created.json"))
            .expect("event parsed");
        insta::assert_debug_snapshot!("webhook.check_run.created", event);

        let event = parse_event(
            "check_run".to_string(),
            include_bytes!("mock/webhook.check_run.completed.json"),
        )
        .expect("event parsed");
        insta::assert_debug_snapshot!("webhook.check_run.completed", event);

        let event = parse_event("push".to_string(), include_bytes!("mock/webhook.push.json")).expect("event parsed");
        insta::assert_debug_snapshot!("webhook.push", event);

        let event = parse_event(
            "pull_request".to_string(),
            include_bytes!("mock/webhook.pull_request.synchronize.json"),
        )
        .expect("event parsed");
        insta::assert_debug_snapshot!("webhook.pull_request.synchronize", event);

        let event = parse_event(
            "issue_comment".to_string(),
            include_bytes!("mock/webhook.issue_comment.created.json"),
        )
        .expect("event parsed");
        insta::assert_debug_snapshot!("webhook.issue_comment.created", event);

        let event = parse_event("delete".to_string(), include_bytes!("mock/webhook.delete.json")).expect("event parsed");
        insta::assert_debug_snapshot!("webhook.delete", event);

        let event = parse_event("create".to_string(), include_bytes!("mock/webhook.create.json")).expect("event parsed");
        insta::assert_debug_snapshot!("webhook.create", event);

        let event = parse_event(
            "pull_request_review".to_string(),
            include_bytes!("mock/webhook.pull_request_review.submitted.json"),
        )
        .expect("event parsed");
        insta::assert_debug_snapshot!("webhook.pull_request_review.submitted", event);

        let event = parse_event(
            "issue_comment".to_string(),
            include_bytes!("mock/webhook.issue_comment.created-brawl-cmd.json"),
        )
        .expect("event parsed");
        insta::assert_debug_snapshot!("webhook.issue_comment.created-brawl-cmd", event);

        let event = parse_event(
            "installation_repositories".to_string(),
            include_bytes!("mock/webhook.installation_repositories.added.json"),
        )
        .expect("event parsed");
        insta::assert_debug_snapshot!("webhook.installation_repositories.added", event);

        let event = parse_event(
            "installation_repositories".to_string(),
            include_bytes!("mock/webhook.installation_repositories.removed.json"),
        )
        .expect("event parsed");
        insta::assert_debug_snapshot!("webhook.installation_repositories.removed", event);

        let event = parse_event(
            "repository".to_string(),
            include_bytes!("mock/webhook.repository.deleted.json"),
        )
        .expect("event parsed");
        insta::assert_debug_snapshot!("webhook.repository.deleted", event);

        let event = parse_event(
            "repository".to_string(),
            include_bytes!("mock/webhook.repository.created.json"),
        )
        .expect("event parsed");
        insta::assert_debug_snapshot!("webhook.repository.created", event);

        let event = parse_event(
            "installation".to_string(),
            include_bytes!("mock/webhook.installation.suspend.json"),
        )
        .expect("event parsed");
        insta::assert_debug_snapshot!("webhook.installation.suspend", event);

        let event = parse_event(
            "installation".to_string(),
            include_bytes!("mock/webhook.installation.unsuspend.json"),
        )
        .expect("event parsed");
        insta::assert_debug_snapshot!("webhook.installation.unsuspend", event);
    }

    #[test]
    fn test_verify_gh_signature() {
        let mut mac = Hmac::<Sha256>::new_from_slice("secret".as_bytes()).expect("Cannot create HMAC key");
        mac.update(b"body");
        let signature = mac.finalize().into_bytes().to_vec();
        let signature = hex::encode(signature);
        let signature = format!("sha256={}", signature);

        let mut headers = HeaderMap::new();
        headers.insert("X-Hub-Signature-256", signature.parse().unwrap());
        assert!(verify_gh_signature(&headers, b"body", "secret"));
    }

    #[tokio::test]
    async fn test_parse_from_request() {
        let data = include_bytes!("mock/webhook.push.json");

        let mut mac = Hmac::<Sha256>::new_from_slice("secret".as_bytes()).expect("Cannot create HMAC key");
        mac.update(data);
        let signature = mac.finalize().into_bytes().to_vec();
        let signature = hex::encode(signature);
        let signature = format!("sha256={}", signature);

        let request = Request::builder()
            .header("X-Hub-Signature-256", signature)
            .header("X-GitHub-Event", "push")
            .body(Body::from(data.to_vec()))
            .unwrap();

        parse_from_request(request, "secret").await.expect("event parsed");
    }

    #[tokio::test]
    async fn test_parse_from_request_invalid_signature() {
        let data = include_bytes!("mock/webhook.push.json");
        let request = Request::builder()
            .header("X-GitHub-Event", "push")
            .body(Body::from(data.to_vec()))
            .unwrap();
        let err = parse_from_request(request, "secret").await.expect_err("error");
        assert_eq!(err, (StatusCode::UNAUTHORIZED, "Invalid signature".to_string()));
    }

    #[tokio::test]
    async fn test_parse_from_request_missing_header() {
        let data = include_bytes!("mock/webhook.push.json");
        let request = Request::builder().body(Body::from(data.to_vec())).unwrap();
        let err = parse_from_request(request, "secret").await.expect_err("error");
        assert_eq!(err, (StatusCode::BAD_REQUEST, "Missing X-GitHub-Event header".to_string()));
    }

    #[tokio::test]
    async fn test_parse_from_request_invalid_body() {
        let body = Body::from(vec![0; 1024 * 1024 * 20]);
        let request = Request::builder().header("X-GitHub-Event", "push").body(body).unwrap();
        let err = parse_from_request(request, "secret").await.expect_err("error");
        assert_eq!(err, (StatusCode::BAD_REQUEST, "Failed to read body".to_string()));
    }
}
