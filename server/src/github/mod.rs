use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use installation::InstallationClient;
use models::Installation;
use octocrab::models::{AppId, InstallationId, RepositoryId, UserId};
use octocrab::Octocrab;

pub mod config;
pub mod installation;
pub mod label_state;
pub mod merge_workflow;
pub mod messages;
pub mod models;
pub mod repo;
pub mod repo_lock;

pub struct GitHubService {
    client: Octocrab,
    installations: parking_lot::Mutex<HashMap<InstallationId, Arc<InstallationClient>>>,
}

impl GitHubService {
    /// Create a new GitHubService with a new Octocrab client
    pub async fn new(app_id: AppId, key: jsonwebtoken::EncodingKey) -> anyhow::Result<Self> {
        let client = Octocrab::builder()
            .app(app_id, key)
            .build()
            .context("build octocrab client")?;

        Self::new_with_client(client).await
    }

    pub async fn new_with_client(client: Octocrab) -> anyhow::Result<Self> {
        let mut installations = HashMap::new();
        let mut user_to_installation = HashMap::new();

        let installs = client
            .all_pages(client.apps().installations().send().await.context("get installations")?)
            .await?;

        for installation in installs {
            let client = client.installation(installation.id).context("build installation client")?;
            let installation_id = installation.id;
            let account_id = installation.account.id;

            let client = Arc::new(InstallationClient::new(client, installation.clone().into()));

            if let Err(err) = client.fetch_repositories().await {
                tracing::error!(
                    "error fetching repositories for installation: {} ({}): {}",
                    installation.account.login,
                    installation_id,
                    err
                );
            }

            user_to_installation.insert(account_id, installation_id);
            installations.insert(installation_id, client);
        }

        Ok(Self {
            client,
            installations: parking_lot::Mutex::new(installations),
        })
    }

    /// Get an installation client by installation id
    pub fn get_client(&self, installation_id: InstallationId) -> Option<Arc<InstallationClient>> {
        self.installations.lock().get(&installation_id).cloned()
    }

    /// Get an installation client by user id
    pub fn get_client_by_user(&self, user_id: UserId) -> Option<Arc<InstallationClient>> {
        self.installations
            .lock()
            .values()
            .find(|client| client.installation().account.id == user_id)
            .cloned()
    }

    /// Get an installation client by repository id
    pub fn get_client_by_repo(&self, repo_id: RepositoryId) -> Option<Arc<InstallationClient>> {
        self.installations
            .lock()
            .values()
            .find(|client| client.has_repository(repo_id))
            .cloned()
    }

    /// Get all installation clients
    pub fn installations(&self) -> HashMap<InstallationId, Arc<InstallationClient>> {
        self.installations.lock().clone()
    }

    /// Update an installation client
    pub async fn update_installation(&self, installation: Installation) -> anyhow::Result<()> {
        let install = self.installations.lock().get(&installation.id).cloned();
        if let Some(install) = install {
            install.update_installation(installation.clone());
            if let Err(err) = install.fetch_repositories().await {
                tracing::error!(
                    "error fetching repositories for installation: {} ({}): {}",
                    installation.account.login,
                    installation.id,
                    err
                );
            }
        } else {
            let installation_id = installation.id;
            let client = self
                .client
                .installation(installation_id)
                .context("build installation client")?;

            let client = Arc::new(InstallationClient::new(client, installation.clone()));

            if let Err(err) = client.fetch_repositories().await {
                tracing::error!(
                    "error fetching repositories for installation: {} ({}): {}",
                    installation.account.login,
                    installation_id,
                    err
                );
            }

            self.installations.lock().insert(installation_id, client);
        }

        Ok(())
    }

    /// Delete an installation client
    pub fn delete_installation(&self, installation_id: InstallationId) {
        self.installations.lock().remove(&installation_id);
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod test_utils {
    use axum::body::HttpBody;
    use bytes::Bytes;
    use http::{Method, Request, Response};
    use http_body_util::{BodyExt, Full};
    use jsonwebtoken::EncodingKey;
    use octocrab::auth::AppAuth;
    use octocrab::models::{AppId, RepositoryId, UserId};
    use octocrab::{AuthState, Octocrab, OctocrabBuilder};

    use super::installation::OrgState;
    use super::merge_workflow::GitHubMergeWorkflow;
    use super::models::{Repository, User};
    use super::repo::RepoClient;

    pub struct MockMergeWorkflow(pub u32);

    impl GitHubMergeWorkflow for MockMergeWorkflow {}

    /// A dumped RSA key used to test the repo client
    const KEY: &[u8] = &[
        48, 130, 4, 162, 2, 1, 0, 2, 130, 1, 1, 0, 183, 247, 207, 88, 52, 41, 116, 208, 34, 231, 152, 10, 86, 112, 121, 79,
        92, 101, 50, 238, 236, 99, 6, 126, 76, 239, 102, 222, 235, 53, 252, 190, 7, 119, 143, 19, 193, 124, 50, 14, 176,
        109, 67, 119, 183, 10, 25, 83, 5, 106, 196, 164, 125, 197, 12, 105, 89, 6, 172, 33, 152, 128, 76, 182, 130, 63, 19,
        123, 91, 203, 109, 77, 48, 247, 88, 21, 204, 39, 169, 171, 188, 89, 24, 200, 26, 13, 8, 54, 27, 66, 161, 35, 230,
        251, 39, 49, 234, 154, 35, 186, 159, 153, 171, 254, 102, 2, 156, 39, 255, 149, 199, 223, 117, 70, 162, 238, 59, 184,
        195, 129, 242, 153, 164, 153, 218, 51, 23, 195, 72, 235, 245, 170, 22, 226, 143, 39, 138, 127, 184, 66, 37, 179,
        121, 243, 134, 106, 98, 132, 206, 8, 200, 220, 214, 209, 85, 94, 21, 65, 225, 184, 68, 81, 253, 206, 45, 225, 73,
        176, 78, 251, 52, 251, 96, 199, 132, 107, 13, 199, 96, 113, 153, 108, 188, 81, 61, 65, 30, 130, 234, 88, 181, 142,
        173, 175, 124, 115, 57, 190, 39, 44, 171, 215, 20, 244, 116, 79, 215, 40, 73, 3, 167, 130, 107, 30, 94, 148, 153,
        198, 116, 31, 235, 243, 192, 16, 2, 77, 181, 8, 146, 120, 231, 180, 52, 35, 98, 136, 13, 52, 44, 156, 182, 82, 141,
        185, 61, 45, 5, 157, 120, 253, 90, 154, 90, 12, 196, 39, 2, 3, 1, 0, 1, 2, 130, 1, 0, 2, 123, 65, 60, 187, 87, 99,
        207, 250, 232, 140, 208, 118, 226, 5, 128, 224, 138, 44, 233, 180, 30, 145, 211, 218, 77, 208, 97, 105, 98, 205, 9,
        243, 39, 213, 178, 58, 133, 230, 86, 244, 98, 68, 234, 180, 121, 90, 102, 24, 72, 156, 102, 107, 155, 224, 210, 250,
        244, 112, 21, 243, 236, 167, 28, 63, 29, 130, 177, 195, 71, 55, 46, 55, 94, 222, 189, 76, 135, 172, 110, 56, 152,
        43, 17, 103, 232, 141, 23, 205, 190, 84, 86, 27, 163, 127, 159, 216, 190, 67, 133, 28, 234, 1, 187, 232, 188, 88,
        70, 225, 215, 175, 94, 128, 66, 119, 168, 38, 254, 19, 5, 9, 180, 176, 215, 178, 109, 198, 0, 66, 121, 42, 169, 107,
        153, 137, 88, 189, 98, 64, 36, 51, 119, 10, 101, 242, 232, 239, 111, 67, 176, 87, 144, 97, 163, 120, 112, 179, 193,
        72, 193, 5, 71, 91, 8, 34, 246, 159, 69, 214, 10, 69, 206, 235, 60, 77, 91, 31, 107, 12, 132, 18, 14, 119, 162, 115,
        115, 74, 246, 25, 119, 190, 66, 34, 87, 156, 192, 166, 213, 113, 232, 11, 105, 67, 85, 104, 31, 237, 103, 79, 139,
        226, 245, 136, 213, 224, 40, 104, 149, 68, 188, 129, 197, 169, 123, 34, 20, 61, 181, 177, 170, 209, 217, 26, 158,
        19, 251, 133, 215, 51, 199, 47, 104, 33, 166, 218, 104, 111, 179, 104, 247, 25, 97, 127, 143, 26, 65, 2, 129, 129,
        0, 232, 160, 13, 164, 43, 14, 34, 48, 142, 115, 198, 101, 51, 1, 109, 238, 62, 44, 12, 181, 167, 37, 193, 125, 67,
        51, 227, 212, 122, 231, 207, 167, 123, 124, 184, 224, 186, 196, 189, 54, 90, 217, 51, 0, 133, 10, 32, 3, 139, 67,
        26, 74, 135, 137, 214, 249, 156, 25, 238, 100, 99, 165, 183, 68, 151, 220, 99, 99, 240, 183, 31, 200, 82, 22, 105,
        117, 170, 153, 236, 145, 107, 159, 94, 19, 1, 26, 48, 144, 85, 27, 252, 0, 175, 184, 58, 135, 60, 123, 204, 50, 219,
        211, 153, 140, 44, 44, 50, 143, 59, 22, 90, 16, 204, 68, 9, 234, 201, 135, 195, 96, 112, 71, 202, 115, 34, 91, 205,
        63, 2, 129, 129, 0, 202, 116, 30, 224, 50, 202, 226, 117, 157, 90, 69, 182, 213, 244, 177, 97, 146, 179, 207, 97,
        92, 73, 46, 140, 103, 163, 202, 174, 157, 64, 112, 98, 112, 176, 28, 136, 23, 245, 172, 199, 47, 45, 162, 165, 254,
        183, 109, 214, 125, 65, 27, 140, 66, 246, 25, 51, 252, 40, 206, 105, 178, 47, 4, 178, 42, 43, 101, 111, 140, 195,
        161, 6, 36, 22, 192, 187, 46, 26, 64, 135, 135, 231, 157, 149, 179, 42, 30, 75, 223, 102, 230, 207, 189, 87, 137,
        239, 254, 194, 233, 110, 204, 129, 137, 65, 97, 101, 163, 88, 112, 6, 47, 174, 230, 199, 140, 53, 135, 139, 41, 245,
        5, 170, 117, 86, 112, 71, 7, 25, 2, 129, 128, 39, 192, 73, 244, 118, 195, 8, 134, 161, 161, 25, 18, 235, 255, 95,
        136, 169, 169, 31, 86, 223, 68, 45, 103, 57, 87, 161, 164, 10, 136, 152, 76, 119, 102, 157, 181, 17, 85, 83, 59,
        249, 148, 74, 9, 217, 178, 28, 60, 94, 204, 205, 174, 84, 176, 242, 66, 95, 49, 115, 50, 70, 112, 231, 251, 89, 179,
        248, 107, 248, 147, 98, 99, 249, 219, 8, 148, 105, 221, 185, 182, 51, 220, 220, 215, 132, 133, 180, 44, 197, 206,
        109, 102, 180, 160, 87, 168, 10, 102, 225, 67, 3, 155, 138, 14, 144, 241, 208, 133, 247, 67, 223, 138, 37, 77, 175,
        32, 38, 230, 3, 53, 244, 153, 223, 247, 130, 180, 139, 67, 2, 129, 128, 107, 5, 63, 93, 28, 252, 139, 1, 201, 144,
        114, 209, 216, 0, 101, 212, 66, 140, 178, 207, 176, 205, 46, 194, 33, 247, 63, 169, 86, 143, 61, 217, 139, 224, 76,
        244, 212, 85, 150, 100, 36, 216, 102, 230, 128, 227, 206, 56, 88, 54, 22, 173, 234, 167, 213, 98, 217, 165, 104,
        152, 15, 13, 51, 218, 74, 216, 109, 226, 173, 242, 172, 40, 102, 227, 112, 54, 130, 132, 118, 32, 47, 3, 141, 22,
        25, 131, 230, 72, 13, 108, 132, 14, 196, 244, 133, 130, 76, 150, 20, 119, 241, 187, 120, 39, 11, 169, 130, 211, 185,
        68, 75, 232, 149, 46, 95, 59, 220, 206, 255, 250, 250, 103, 197, 103, 80, 42, 251, 225, 2, 129, 128, 0, 213, 199,
        194, 63, 91, 177, 199, 223, 166, 82, 248, 176, 12, 224, 124, 126, 163, 175, 123, 52, 203, 73, 148, 171, 98, 70, 214,
        137, 67, 38, 99, 177, 62, 182, 130, 65, 24, 196, 125, 153, 44, 59, 228, 192, 74, 183, 70, 179, 194, 139, 124, 160,
        115, 150, 182, 214, 183, 142, 7, 191, 192, 103, 105, 213, 46, 88, 160, 216, 141, 229, 90, 191, 42, 5, 168, 34, 210,
        89, 32, 2, 47, 56, 116, 114, 52, 109, 122, 255, 133, 181, 170, 155, 100, 0, 221, 18, 48, 120, 11, 7, 138, 140, 65,
        226, 28, 93, 130, 183, 175, 26, 187, 60, 184, 30, 60, 130, 86, 85, 138, 215, 117, 247, 134, 183, 82, 55, 13,
    ];

    pub fn mock_octocrab() -> (
        Octocrab,
        tower_test::mock::Handle<Request<impl HttpBody + Send + Sync>, Response<Full<Bytes>>>,
    ) {
        let (service, handle) = tower_test::mock::pair();

        let client = OctocrabBuilder::new_empty()
            .with_service(service)
            .with_auth(AuthState::App(AppAuth {
                app_id: AppId(1),
                key: EncodingKey::from_rsa_der(KEY),
            }))
            .build()
            .unwrap();

        (client, handle)
    }

    pub async fn mock_repo_client(
        octocrab: Octocrab,
        repo: Repository,
        merge_workflow: MockMergeWorkflow,
        base_commit_sha: Option<String>,
    ) -> RepoClient<MockMergeWorkflow> {
        RepoClient::new(repo, octocrab, OrgState::default(), merge_workflow, base_commit_sha)
    }

    pub fn default_repo() -> Repository {
        Repository {
            id: RepositoryId(899726767),
            name: "ci-testing".to_owned(),
            owner: User {
                login: "ScuffleCloud".to_owned(),
                id: UserId(122814584),
            },
            default_branch: Some("main".to_string()),
        }
    }

    #[derive(Debug)]
    #[allow(unused)]
    pub struct DebugReq {
        pub method: Method,
        pub uri: String,
        pub headers: Vec<(String, String)>,
        pub body: Option<serde_json::Value>,
    }

    pub async fn debug_req(req: Request<impl HttpBody + Send + Sync>) -> DebugReq {
        let (parts, body) = req.into_parts();
        let body = match body.collect().await {
            Ok(body) => body,
            Err(_) => unreachable!("body error"),
        };

        let body = body.to_bytes();

        DebugReq {
            method: parts.method,
            uri: parts.uri.to_string(),
            headers: parts
                .headers
                .iter()
                .map(|(k, v)| {
                    let k = k.to_string();
                    let v = v.to_str().unwrap().to_string();
                    if k.eq_ignore_ascii_case("authorization") {
                        (k, "REDACTED".to_string())
                    } else {
                        (k, v)
                    }
                })
                .collect(),
            body: if body.is_empty() {
                None
            } else {
                Some(serde_json::from_slice(&body).expect("body is json"))
            },
        }
    }

    pub fn mock_response(status: http::StatusCode, body: &'static [u8]) -> Response<Full<Bytes>> {
        Response::builder()
            .status(status)
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from_static(body)))
            .unwrap()
    }
}

#[cfg(test)]
mod test {
    use http::StatusCode;
    use models::User;
    use test_utils::{debug_req, mock_octocrab, mock_response};

    use super::*;

    #[tokio::test]
    async fn test_new_with_client() {
        let (octocrab, mut handle) = mock_octocrab();
        let service = tokio::spawn(async move { GitHubService::new_with_client(octocrab).await.unwrap() });

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: GET,
            uri: "/app/installations?",
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

        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/installations.json")));

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: POST,
            uri: "/app/installations/1/access_tokens",
            headers: [
                (
                    "authorization",
                    "REDACTED",
                ),
            ],
            body: Some(
                Object {},
            ),
        }
        "#);

        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/access_token.json")));

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: GET,
            uri: "/installation/repositories?per_page=100&page=1",
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
            include_bytes!("mock/get_installation_repos.json"),
        ));

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: GET,
            uri: "/repositories/1296269/git/ref/heads/master",
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

        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/get_ref.json")));

        let service = service.await.unwrap();

        assert!(service.get_client(InstallationId(1)).is_some());
        assert!(service.get_client(InstallationId(2)).is_none());
        assert!(service.get_client_by_user(UserId(1)).is_some());
        assert!(service.get_client_by_user(UserId(2)).is_none());
        assert!(service.get_client_by_repo(RepositoryId(1296269)).is_some());
        assert!(service.get_client_by_repo(RepositoryId(1296270)).is_none());
        assert!(service.installations().len() == 1);

        let task = tokio::spawn(async move {
            service
                .update_installation(Installation {
                    id: InstallationId(1),
                    account: User {
                        login: "troykomodo".to_owned(),
                        id: UserId(122814584),
                    },
                })
                .await
                .unwrap();

            service
                .update_installation(Installation {
                    id: InstallationId(2),
                    account: User {
                        login: "troykomodo2".to_owned(),
                        id: UserId(122814585),
                    },
                })
                .await
                .unwrap();

            service
        });

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: GET,
            uri: "/installation/repositories?per_page=100&page=1",
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
            include_bytes!("mock/get_installation_repos.json"),
        ));

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: GET,
            uri: "/repositories/1296269/git/ref/heads/master",
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

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: POST,
            uri: "/app/installations/2/access_tokens",
            headers: [
                (
                    "authorization",
                    "REDACTED",
                ),
            ],
            body: Some(
                Object {},
            ),
        }
        "#);

        resp.send_response(mock_response(StatusCode::OK, include_bytes!("mock/access_token.json")));

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: GET,
            uri: "/installation/repositories?per_page=100&page=1",
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
            include_bytes!("mock/get_installation_repos.json"),
        ));

        let (req, resp) = handle.next_request().await.unwrap();
        insta::assert_debug_snapshot!(debug_req(req).await, @r#"
        DebugReq {
            method: GET,
            uri: "/repositories/1296269/git/ref/heads/master",
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

        let service = task.await.unwrap();

        assert!(service.get_client(InstallationId(1)).unwrap().owner() == "troykomodo");
        assert!(service.get_client(InstallationId(2)).unwrap().owner() == "troykomodo2");

        service.delete_installation(InstallationId(1));

        assert!(service.get_client(InstallationId(1)).is_none());
        assert!(service.installations().len() == 1);
        assert!(service.get_client(InstallationId(2)).unwrap().owner() == "troykomodo2");
    }
}
