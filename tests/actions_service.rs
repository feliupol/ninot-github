//! The Actions service endpoints a registered runner calls: token exchange, broker session and
//! message queue, and the run service that hands out and completes a job.

mod common;

use common::Api;
use ninot_github::Server;
use reqwest::blocking::{Client, Response};
use serde_json::{Value, json};

const ORG: &str = "acme";

struct Service<'a> {
    server: &'a Server,
    client: Client,
    token: Option<String>,
}

impl<'a> Service<'a> {
    fn new(server: &'a Server) -> Self {
        Self {
            server,
            client: Client::new(),
            token: None,
        }
    }

    fn send(&self, method: &str, path: &str, body: Option<Value>) -> Response {
        let url = format!("{}/_services/{path}", self.server.url());
        let mut request = self.client.request(method.parse().unwrap(), url);
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        request.send().unwrap()
    }

    fn json(&self, method: &str, path: &str, body: Option<Value>) -> (u16, Value) {
        let response = self.send(method, path, body);
        let status = response.status().as_u16();
        (status, response.json().unwrap_or(Value::Null))
    }
}

/// Registers a runner as `config.sh` does and returns its client id.
fn register(server: &Server, name: &str) -> String {
    let api = Api::admin(server);
    let reply = api.post(
        &format!("/orgs/{ORG}/actions/runners/registration-token"),
        json!({}),
    );
    let token = reply.body["token"].as_str().unwrap().to_owned();
    let response = Client::new()
        .post(format!("{}/actions/runners/register", server.url()))
        .header("Authorization", format!("RemoteAuth {token}"))
        .json(&json!({
            "url": format!("https://github.example/{ORG}"),
            "name": name,
            "labels": [{ "name": "self-hosted" }, { "name": "linux" }],
        }))
        .send()
        .unwrap();
    assert_eq!(response.status(), 200);
    let body: Value = response.json().unwrap();
    body["authorization"]["client_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn setup() -> (Server, ninot_github::server::Repository) {
    let server = Server::start().unwrap();
    server.add_org(ORG, &["admin"]);
    let repo = server
        .create_repository(ORG, "ci", true, None, None, "main")
        .unwrap();
    (server, repo)
}

fn exchange(service: &mut Service<'_>, client_id: &str) -> u16 {
    let response = service
        .client
        .post(format!(
            "{}/_services/token/oauth2/token",
            service.server.url()
        ))
        .form(&[
            ("grant_type", "client_credentials"),
            ("client_id", client_id),
        ])
        .send()
        .unwrap();
    let status = response.status().as_u16();
    if status == 200 {
        let body: Value = response.json().unwrap();
        service.token = Some(body["access_token"].as_str().unwrap().to_owned());
    }
    status
}

fn job_state(server: &Server, job: u64) -> Value {
    let reply = Api::admin(server).get(&format!("/repos/{ORG}/ci/actions/jobs/{job}"));
    assert_eq!(reply.status, 200, "{}", reply.body);
    reply.body
}

#[test]
fn rejects_requests_without_a_token_and_unknown_clients() {
    let (server, _) = setup();
    let mut service = Service::new(&server);
    let response = service.send("POST", "broker/session", Some(json!({})));
    assert_eq!(response.status(), 401);
    assert_eq!(response.headers()["www-authenticate"], "Bearer");
    assert_eq!(
        exchange(&mut service, "00000000-0000-4000-8000-0000000000ff"),
        400
    );
}

#[test]
fn a_runner_takes_a_queued_job_and_reports_its_result() {
    let (server, repo) = setup();
    let client_id = register(&server, "r1");
    let job = repo
        .queue_script_job(&["self-hosted"], &["echo hello", "true"])
        .unwrap();
    assert_eq!(job_state(&server, job.id())["status"], "queued");

    let mut service = Service::new(&server);
    assert_eq!(exchange(&mut service, &client_id), 200);
    let (status, session) = service.json("POST", "broker/session", Some(json!({"ownerName": "x"})));
    assert_eq!(status, 200);
    let session_id = session["sessionId"].as_str().unwrap().to_owned();
    let (status, message) = service.json(
        "GET",
        &format!("broker/message?sessionId={session_id}&status=Online"),
        None,
    );
    assert_eq!(status, 200);
    assert_eq!(message["messageType"], "RunnerJobRequest");
    let request: Value = serde_json::from_str(message["body"].as_str().unwrap()).unwrap();
    let request_id = request["runner_request_id"].as_str().unwrap().to_owned();
    assert!(
        request["run_service_url"]
            .as_str()
            .unwrap()
            .ends_with("/_services/run/")
    );

    let (status, acquired) = service.json(
        "POST",
        "run/acquirejob",
        Some(json!({ "jobMessageId": request_id })),
    );
    assert_eq!(status, 200);
    assert_eq!(acquired["messageType"], "RunnerJobRequest");
    assert_eq!(acquired["steps"].as_array().unwrap().len(), 2);
    assert_eq!(
        acquired["steps"][0]["inputs"]["map"][0]["Value"],
        "echo hello"
    );
    let state = job_state(&server, job.id());
    assert_eq!(state["status"], "in_progress");
    assert_eq!(state["runner_name"], "r1");
    let (again, _) = service.json(
        "POST",
        "run/acquirejob",
        Some(json!({ "jobMessageId": request_id })),
    );
    assert_eq!(again, 409);

    let job_guid = acquired["jobId"].clone();
    let (status, renewed) =
        service.json("POST", "run/renewjob", Some(json!({ "jobId": job_guid })));
    assert_eq!(status, 200);
    assert!(renewed["lockedUntil"].is_string());

    let response = service.send(
        "POST",
        "run/completejob",
        Some(json!({
            "jobId": job_guid,
            "conclusion": "succeeded",
            "stepResults": [
                { "name": "Run echo hello", "number": 1, "conclusion": "succeeded" },
                { "name": "Run true", "number": 2, "conclusion": "failed" },
            ],
        })),
    );
    assert_eq!(response.status(), 204);
    let state = job_state(&server, job.id());
    assert_eq!(state["status"], "completed");
    assert_eq!(state["conclusion"], "success");
    assert_eq!(state["steps"][0]["name"], "Run echo hello");
    assert_eq!(state["steps"][1]["conclusion"], "failure");
    let run = Api::admin(&server).get(&format!("/repos/{ORG}/ci/actions/runs/{}", job.run_id()));
    assert_eq!(run.body["status"], "completed");
    assert_eq!(run.body["conclusion"], "success");
    let (gone, _) = service.json("POST", "run/renewjob", Some(json!({ "jobId": job_guid })));
    assert_eq!(gone, 404, "a completed job cannot be renewed");
}

#[test]
fn a_runner_without_the_job_labels_is_offered_nothing() {
    let (server, repo) = setup();
    let client_id = register(&server, "r2");
    repo.queue_script_job(&["gpu"], &["true"]).unwrap();
    let mut service = Service::new(&server);
    exchange(&mut service, &client_id);
    let (_, session) = service.json("POST", "broker/session", Some(json!({})));
    let session_id = session["sessionId"].as_str().unwrap();
    let response = service.send(
        "GET",
        &format!("broker/message?sessionId={session_id}"),
        None,
    );
    assert_eq!(response.status(), 202);
}

#[test]
fn a_session_keeps_the_runner_online_until_it_is_deleted() {
    let (server, _) = setup();
    let client_id = register(&server, "r3");
    let runner_status = || {
        let reply = Api::admin(&server).get(&format!("/orgs/{ORG}/actions/runners"));
        reply.body["runners"][0]["status"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert_eq!(runner_status(), "offline");
    let mut service = Service::new(&server);
    exchange(&mut service, &client_id);
    let (_, session) = service.json("POST", "broker/session", Some(json!({})));
    assert_eq!(runner_status(), "online");
    let id = session["sessionId"].as_str().unwrap();
    assert_eq!(
        service
            .send("DELETE", &format!("broker/session?sessionId={id}"), None)
            .status(),
        204
    );
    assert_eq!(runner_status(), "offline");
}
