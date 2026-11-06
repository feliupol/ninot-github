//! The endpoints the official Actions runner calls after `config.sh`, under `/_services/`.
//!
//! - `token/oauth2/token` exchanges the runner's signed assertion for an access token.
//! - `broker/` holds the runner's session and long-polls for job messages.
//! - `run/` hands the runner a job (`acquirejob`), extends it (`renewjob`) and takes its
//!   verdict (`completejob`).
//!
//! The assertion's signature is not checked: the client id names the runner. Steps and logs
//! are not uploaded to a results service; the step outcomes in `completejob` are recorded.

use std::sync::Mutex;
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use http::{HeaderMap, Method};
use serde_json::{Value, json};

use crate::api::ApiResponse;
use crate::state::actions_service::{AcquireError, ServiceJob, StepOutcome, guid};
use crate::state::{State, lock};
use crate::types;

/// How long a message poll waits for a job before answering that there is none.
const POLL_TIMEOUT: Duration = Duration::from_secs(3);
const POLL_INTERVAL: Duration = Duration::from_millis(100);
/// The distributed-task resource area. A connection whose service owner is this area serves it
/// itself, so the runner needs no service definitions to find it.
const DISTRIBUTED_TASK_AREA: &str = "a85b8835-c1a1-4aac-ae97-1c3d0ba72dbd";
const SERVICES: &str = "/_services/";

/// Answers a request under `/_services/`, or `None` for any other request.
pub(crate) fn serve(
    state: &Mutex<State>,
    method: &Method,
    headers: &HeaderMap,
    path: &str,
    query: Option<&str>,
    body: &[u8],
    web_url: &str,
) -> Option<ApiResponse> {
    let service_path = path.strip_prefix(SERVICES)?;
    if method == Method::POST && service_path == "token/oauth2/token" {
        return Some(exchange_token(state, body));
    }
    let token = headers
        .get(http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim);
    let runner_id = token.and_then(|token| lock(state).access_token_runner(token));
    let Some(runner_id) = runner_id else {
        return Some(
            ApiResponse::error(401, "Bad credentials")
                .with_header("www-authenticate", "Bearer".to_owned()),
        );
    };
    let query: Vec<(String, String)> = url::form_urlencoded::parse(query.unwrap_or("").as_bytes())
        .into_owned()
        .collect();
    let param = |name: &str| {
        query
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let token = token.unwrap_or_default();
    let response = match (method.as_str(), service_path) {
        ("GET", "pipelines/_apis/connectionData") => connection_data(),
        ("POST", "broker/session") => create_session(state, runner_id, body),
        ("DELETE", "broker/session") => {
            lock(state).close_session(param("sessionId").unwrap_or_default());
            ApiResponse::no_content()
        }
        ("GET", "broker/message") => poll_message(state, runner_id, param("sessionId"), web_url),
        ("POST", "broker/acknowledge") => ApiResponse::ok(json!({})),
        ("POST", "run/acquirejob") => acquire_job(state, runner_id, token, body, web_url),
        ("POST", "run/renewjob") => renew_job(state, body),
        ("POST", "run/completejob") => complete_job(state, body),
        _ => ApiResponse::not_found(),
    };
    Some(response)
}

/// `GET pipelines/_apis/connectionData`: the runner connects to its configured server first.
fn connection_data() -> ApiResponse {
    ApiResponse::ok(json!({
        "instanceId": guid(8, 1),
        "locationServiceData": {
            "serviceOwner": DISTRIBUTED_TASK_AREA,
            "defaultAccessMappingMoniker": "PublicAccessMapping",
            "lastChangeId": 1,
            "lastChangeId64": 1,
            "accessMappings": [],
            "serviceDefinitions": [],
        },
    }))
}

fn parse(body: &[u8]) -> Value {
    serde_json::from_slice(body).unwrap_or(Value::Null)
}

/// The runner id inside a client id such as `00000000-0000-4000-8000-00000000002a`.
fn runner_of_client(client_id: &str) -> Option<u64> {
    u64::from_str_radix(client_id.rsplit('-').next()?, 16).ok()
}

/// `POST token/oauth2/token`: the runner sends a JWT whose subject is its client id.
fn exchange_token(state: &Mutex<State>, body: &[u8]) -> ApiResponse {
    let form: Vec<(String, String)> = url::form_urlencoded::parse(body).into_owned().collect();
    let field = |name: &str| {
        form.iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let subject = field("client_assertion")
        .and_then(|jwt| jwt.split('.').nth(1))
        .and_then(|payload| URL_SAFE_NO_PAD.decode(payload).ok())
        .and_then(|payload| serde_json::from_slice::<Value>(&payload).ok())
        .and_then(|claims| claims.get("sub").and_then(Value::as_str).map(str::to_owned))
        .or_else(|| field("client_id").map(str::to_owned));
    let mut state = lock(state);
    let Some(runner_id) = subject
        .as_deref()
        .and_then(runner_of_client)
        .filter(|id| state.runner_by_id(*id).is_some())
    else {
        return ApiResponse::json(400, json!({ "error": "invalid_client" }));
    };
    let token = access_token(guid(9, state.next_service_id()));
    state.register_access_token(runner_id, token.clone());
    ApiResponse::ok(json!({
        "access_token": token,
        "token_type": "access_token",
        "expires_in": 3600,
    }))
}

/// An access token. The runner reads claims from the JWT it is given, so it has the JWT shape;
/// it is an opaque handle to the fake, which does not check the signature.
fn access_token(id: String) -> String {
    let now = crate::time::unix_now();
    let encode = |value: Value| URL_SAFE_NO_PAD.encode(value.to_string());
    format!(
        "{}.{}.{}",
        encode(json!({ "typ": "JWT", "alg": "RS256" })),
        encode(json!({
            "iss": "ninot-github",
            "aud": "ninot-github",
            "jti": id,
            "nbf": now,
            "exp": now + 3600,
            "orchid": format!("{id}.job.job"),
        })),
        URL_SAFE_NO_PAD.encode(id.as_bytes()),
    )
}

/// `POST broker/session`: the runner is online while its session lasts.
fn create_session(state: &Mutex<State>, runner_id: u64, body: &[u8]) -> ApiResponse {
    let mut session = parse(body);
    let id = lock(state).open_session(runner_id);
    session["sessionId"] = json!(id);
    ApiResponse::ok(session)
}

/// `GET broker/message`: waits a few seconds for a job, then answers `202` with no message.
fn poll_message(
    state: &Mutex<State>,
    runner_id: u64,
    session_id: Option<&str>,
    web_url: &str,
) -> ApiResponse {
    let waited_from = std::time::Instant::now();
    loop {
        {
            let mut state = lock(state);
            if session_id.is_none_or(|id| state.session_runner(id) != Some(runner_id)) {
                return ApiResponse::json(
                    404,
                    json!({ "errorKind": "RunnerSessionInvalid", "message": "unknown session" }),
                );
            }
            if let Some(job) = state.offer_job(runner_id) {
                let request = json!({
                    "id": guid(2, job.message_id),
                    "runner_request_id": job.message_id.to_string(),
                    "should_acknowledge": true,
                    "run_service_url": format!("{web_url}/_services/run/"),
                    "billing_owner_id": "",
                });
                return ApiResponse::ok(json!({
                    "messageId": job.message_id,
                    "messageType": "RunnerJobRequest",
                    "body": request.to_string(),
                }));
            }
        }
        if waited_from.elapsed() >= POLL_TIMEOUT {
            return ApiResponse::bytes(202, "application/json", Vec::new());
        }
        std::thread::sleep(POLL_INTERVAL);
    }
}

fn service_error(status: u16, message: &str) -> ApiResponse {
    ApiResponse::json(
        status,
        json!({
            "source": "actions-run-service",
            "statusCode": status,
            "errorMessage": message,
        }),
    )
}

/// `POST run/acquirejob`: the runner takes the offered job and gets its definition.
fn acquire_job(
    state: &Mutex<State>,
    runner_id: u64,
    token: &str,
    body: &[u8],
    web_url: &str,
) -> ApiResponse {
    let body = parse(body);
    let Some(message_id) = body
        .get("jobMessageId")
        .and_then(Value::as_str)
        .and_then(|id| id.parse::<u64>().ok())
    else {
        return service_error(404, "job message not found");
    };
    match lock(state).acquire_job(message_id, runner_id) {
        Ok(job) => ApiResponse::ok(job_message(&job, token, web_url)),
        Err(AcquireError::NotFound) => service_error(404, "job message not found"),
        Err(AcquireError::AlreadyAcquired) => service_error(409, "job already acquired"),
    }
}

/// The job id a request names by its `jobId` GUID.
fn job_of(body: &Value) -> Option<u64> {
    u64::from_str_radix(body.get("jobId")?.as_str()?.rsplit('-').next()?, 16).ok()
}

/// `POST run/renewjob`: the lock lasts another ten minutes.
fn renew_job(state: &Mutex<State>, body: &[u8]) -> ApiResponse {
    let body = parse(body);
    let acquired = job_of(&body).and_then(|id| lock(state).acquired_job(id));
    if acquired.is_none() {
        return service_error(404, "job not found");
    }
    ApiResponse::ok(json!({ "lockedUntil": locked_until() }))
}

/// `POST run/completejob`: records the conclusion and the outcome of each step.
fn complete_job(state: &Mutex<State>, body: &[u8]) -> ApiResponse {
    let body = parse(body);
    let Some(job_id) = job_of(&body) else {
        return service_error(404, "job not found");
    };
    let conclusion = match body
        .get("conclusion")
        .and_then(Value::as_str)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("succeeded" | "succeededwithissues") => types::JobConclusion::Success,
        Some("canceled") => types::JobConclusion::Cancelled,
        Some("skipped") => types::JobConclusion::Skipped,
        _ => types::JobConclusion::Failure,
    };
    let steps: Vec<StepOutcome> = body
        .get("stepResults")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, step)| StepOutcome {
            name: step
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            number: step
                .get("number")
                .and_then(Value::as_i64)
                .unwrap_or(i64::try_from(index + 1).unwrap()),
            conclusion: match step
                .get("conclusion")
                .and_then(Value::as_str)
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("succeeded" | "succeededwithissues") => "success",
                Some("canceled") => "cancelled",
                Some("skipped") => "skipped",
                _ => "failure",
            }
            .to_owned(),
        })
        .collect();
    match lock(state).complete_job(job_id, conclusion, &steps) {
        Some(_) => ApiResponse::no_content(),
        None => service_error(404, "job not found"),
    }
}

fn locked_until() -> String {
    crate::time::format_timestamp(crate::time::unix_now() + 600)
}

fn string_map(entries: &[(&str, String)]) -> Value {
    json!({
        "t": 2,
        "d": entries.iter().map(|(key, value)| json!({ "k": key, "v": value })).collect::<Vec<_>>(),
    })
}

/// The `AgentJobRequestMessage` for a job of `shell` steps.
fn job_message(job: &ServiceJob, token: &str, web_url: &str) -> Value {
    let repository = format!("{}/{}", job.owner, job.repo);
    let github = string_map(&[
        ("repository", repository.clone()),
        ("repository_owner", job.owner.clone()),
        ("job", "job".to_owned()),
        ("event_name", "push".to_owned()),
        ("run_id", job.run_id.to_string()),
        ("run_number", "1".to_owned()),
        ("run_attempt", "1".to_owned()),
        ("actor", job.owner.clone()),
        ("workflow", "ci".to_owned()),
        ("sha", "0".repeat(40)),
        ("ref", "refs/heads/main".to_owned()),
        ("ref_name", "main".to_owned()),
        ("ref_type", "branch".to_owned()),
        ("server_url", web_url.to_owned()),
        ("api_url", format!("{web_url}/api/v3")),
        ("graphql_url", format!("{web_url}/api/graphql")),
        ("token", token.to_owned()),
    ]);
    let steps: Vec<Value> = job
        .scripts
        .iter()
        .enumerate()
        .map(|(index, script)| {
            json!({
                "type": "action",
                "id": guid(4, job.job_id * 1000 + index as u64),
                "name": format!("__run_{index}"),
                "contextName": format!("__run_{index}"),
                "reference": { "type": "script" },
                "inputs": { "type": 2, "map": [{ "Key": "script", "Value": script }] },
                "condition": "success()",
            })
        })
        .collect();
    json!({
        "messageType": "RunnerJobRequest",
        "plan": {
            "scopeIdentifier": guid(5, 1),
            "planType": "free",
            "version": 12,
            "planId": guid(6, job.job_id),
        },
        "timeline": { "id": guid(7, job.job_id), "changeId": 1 },
        "jobId": guid(3, job.job_id),
        "jobDisplayName": "job",
        "jobName": "job",
        "requestId": job.message_id,
        "lockedUntil": locked_until(),
        "resources": {
            "endpoints": [{
                "name": "SystemVssConnection",
                "url": format!("{web_url}/_services/run/"),
                "authorization": { "scheme": "OAuth", "parameters": { "AccessToken": token } },
                "data": {},
            }],
        },
        "contextData": {
            "github": github,
            "needs": { "t": 2 },
            "strategy": { "t": 2 },
            "matrix": { "t": 2 },
            "secrets": { "t": 2 },
            "vars": { "t": 2 },
            "inputs": { "t": 2 },
        },
        "variables": {
            "system.github.job": { "value": "job" },
            "system.github.token": { "value": token, "isSecret": true },
            "system.runnerGroupName": { "value": "Default" },
        },
        "steps": steps,
        "mask": [],
        "fileTable": [],
        "workspace": {},
    })
}
