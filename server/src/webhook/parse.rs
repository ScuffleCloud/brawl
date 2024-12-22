use axum::body::Body;
use axum::http::{HeaderMap, HeaderValue, Request, StatusCode};
use hmac::{Hmac, Mac};
use octocrab::models::webhook_events::{EventInstallation, WebhookEventPayload, WebhookEventType};
use octocrab::models::InstallationId;
use sha2::Sha256;

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
pub struct WebhookEvent {
    pub sender: Option<User>,
    pub repository: Option<Repository>,
    pub installtion_id: Option<InstallationId>,
    pub installation: Option<Installation>,
    pub kind: WebhookEventType,
    pub specific: WebhookEventPayload,
}

fn parse_event(header: String, body: &[u8]) -> Result<WebhookEvent, (StatusCode, String)> {
    // NOTE: this is inefficient code to simply reuse the code from "derived"
    // serde::Deserialize instead of writing specific deserialization code for the
    // enum.
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

    Ok(WebhookEvent {
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
    })
}

pub async fn parse_from_request(request: Request<Body>, secret: &str) -> Result<WebhookEvent, (StatusCode, String)> {
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
