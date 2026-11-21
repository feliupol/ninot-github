//! Replays the octokit/fixtures recordings in `tests/fixtures/octokit/` against the fake.
//!
//! Each test seeds the state its scenario's `record.js` set up before recording, sends the
//! recorded requests in order and compares every response with the recording: status, the
//! `location` header, and the body's keys, JSON types, nesting and array lengths. Values are
//! compared only where the scenario's `test.js` asserts them, since octokit normalizes ids,
//! node ids, timestamps, counts and shas. Recorded ids and shas in request paths and bodies are
//! translated to the fake's, learned from the seed and from earlier responses.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::OnceLock;

use ninot_github::{IMPLEMENTED, OPERATIONS, OperationInfo, Repository, Server};
use reqwest::blocking::Client;
use reqwest::redirect::Policy;
use serde_json::{Map, Value, json};

const ORG: &str = "octokit-fixture-org";
const USER_A: &str = "octokit-fixture-user-a";
const USER_B: &str = "octokit-fixture-user-b";
/// Scopes of the recording tokens, from the recorded `x-oauth-scopes` header.
const SCOPES: &[&str] = &[
    "admin:gpg_key",
    "admin:org",
    "admin:org_hook",
    "admin:public_key",
    "admin:repo_hook",
    "delete_repo",
    "gist",
    "notifications",
    "repo",
    "user",
    "workflow",
];

/// Path segments followed by a normalized id or sha, and the entity that id names.
const PATH_IDS: &[(&str, &str)] = &[
    ("repositories", "repository"),
    ("repository_invitations", "invitation"),
    ("releases", "release"),
    ("assets", "release-asset"),
    ("cards", "project-card"),
    ("columns", "project-column"),
    ("statuses", "sha"),
    ("commits", "sha"),
];

/// A recorded request and its response.
struct Exchange {
    scope: String,
    method: String,
    path: String,
    body: Value,
    status: u16,
    request_headers: Map<String, Value>,
    response_headers: Map<String, Value>,
    response: Value,
    account: Option<String>,
}

fn load(scenario: &str) -> Vec<Exchange> {
    let path = Path::new("tests/fixtures/octokit").join(format!("{scenario}.json"));
    let fixture: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    fixture
        .as_array()
        .unwrap()
        .iter()
        .map(|exchange| Exchange {
            scope: exchange["scope"].as_str().unwrap().to_owned(),
            method: exchange["method"].as_str().unwrap().to_uppercase(),
            path: exchange["path"].as_str().unwrap().to_owned(),
            body: exchange["body"].clone(),
            status: exchange["status"].as_u64().unwrap() as u16,
            request_headers: exchange["reqheaders"].as_object().unwrap().clone(),
            response_headers: exchange["headers"].as_object().unwrap().clone(),
            response: exchange["response"].clone(),
            account: exchange["account"].as_str().map(str::to_owned),
        })
        .collect()
}

/// The operation a recorded request calls, preferring the template with most literal segments.
fn operation(method: &str, path: &str) -> Option<&'static OperationInfo> {
    let path = path.split('?').next().unwrap();
    let segments: Vec<&str> = path.split('/').skip(1).collect();
    OPERATIONS
        .iter()
        .filter(|info| info.method.as_str() == method && matches(info.path, &segments))
        .max_by_key(|info| info.path.split('/').filter(|s| !s.starts_with('{')).count())
        .copied()
}

fn matches(template: &str, segments: &[&str]) -> bool {
    let template: Vec<&str> = template.split('/').skip(1).collect();
    for (index, part) in template.iter().enumerate() {
        if *part == "{path}" && index + 1 == template.len() {
            return segments.len() >= template.len();
        }
        match segments.get(index) {
            Some(segment) if part.starts_with('{') => {
                if segment.is_empty() {
                    return false;
                }
            }
            Some(segment) if segment == part => {}
            _ => return false,
        }
    }
    segments.len() == template.len()
}

/// The entity octokit's `to-entity-name.js` names a recorded object, for the entities whose
/// ids appear in request paths.
fn entity(object: &Map<String, Value>) -> Option<&'static str> {
    let url = object.get("url").and_then(Value::as_str).unwrap_or("");
    let url_ends = |kind: &str| {
        url.rsplit_once(kind)
            .is_some_and(|(_, id)| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
    };
    if object.contains_key("forks") || url_ends("/repositories/") {
        Some("repository")
    } else if object.contains_key("invitee") && object.contains_key("inviter") {
        Some("invitation")
    } else if object.contains_key("prerelease") {
        Some("release")
    } else if url_ends("/releases/assets/") {
        Some("release-asset")
    } else if url_ends("/projects/columns/cards/") {
        Some("project-card")
    } else {
        None
    }
}

fn is_placeholder_sha(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|b| b.is_ascii_digit()) && value.starts_with("000")
}

/// Property names the pinned spec documents anywhere in `info`'s `status` response.
///
/// The recordings date from 2022 and the spec is newer, so keys the fake adds that the spec
/// documents, and recorded keys the spec no longer documents, are not differences.
fn documented_keys(info: &OperationInfo, status: u16) -> HashSet<String> {
    static SPEC: OnceLock<Value> = OnceLock::new();
    let spec = SPEC.get_or_init(|| {
        serde_json::from_slice(&std::fs::read("spec/api.github.com.json").unwrap()).unwrap()
    });
    let mut keys = HashSet::new();
    let response = &spec["paths"][info.path][info.method.as_str().to_lowercase()]["responses"]
        [status.to_string()];
    collect_keys(spec, response, &mut HashSet::new(), &mut keys);
    keys
}

fn collect_keys(
    spec: &Value,
    schema: &Value,
    visited: &mut HashSet<String>,
    keys: &mut HashSet<String>,
) {
    match schema {
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                if visited.insert(reference.to_owned()) {
                    let target = spec.pointer(reference.trim_start_matches('#'));
                    collect_keys(spec, target.unwrap_or(&Value::Null), visited, keys);
                }
                return;
            }
            if let Some(Value::Object(properties)) = object.get("properties") {
                keys.extend(properties.keys().cloned());
            }
            for (key, value) in object {
                if !matches!(key.as_str(), "example" | "examples" | "description") {
                    collect_keys(spec, value, visited, keys);
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_keys(spec, item, visited, keys);
            }
        }
        _ => {}
    }
}

fn kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

fn excerpt(value: &Value) -> String {
    let text = value.to_string();
    if text.chars().count() > 60 {
        format!("{}...", text.chars().take(60).collect::<String>())
    } else {
        text
    }
}

/// Reports where `actual` differs from `recorded` in keys, JSON types, nesting or array length.
/// Keys outside `documented`, when given, may only be on the recorded side, and keys in it
/// only on the fake's.
fn compare_shape(
    recorded: &Value,
    actual: &Value,
    pointer: &str,
    documented: Option<&HashSet<String>>,
    diffs: &mut Vec<String>,
) {
    match (recorded, actual) {
        (Value::Object(recorded), Value::Object(actual)) => {
            for (key, value) in recorded {
                match actual.get(key) {
                    Some(other) => {
                        compare_shape(value, other, &format!("{pointer}/{key}"), documented, diffs)
                    }
                    None if documented.is_some_and(|keys| !keys.contains(key)) => {}
                    None => diffs.push(format!(
                        "{pointer}/{key}: missing in the fake, recorded {}",
                        excerpt(value)
                    )),
                }
            }
            let added = actual.keys().filter(|key| {
                !recorded.contains_key(*key) && !documented.is_some_and(|keys| keys.contains(*key))
            });
            for key in added {
                diffs.push(format!(
                    "{pointer}/{key}: not in the recording, fake {}",
                    excerpt(&actual[key])
                ));
            }
        }
        (Value::Array(recorded), Value::Array(actual)) => {
            if recorded.len() != actual.len() {
                diffs.push(format!(
                    "{pointer}: recorded {} items, fake {}",
                    recorded.len(),
                    actual.len()
                ));
            }
            for (index, (recorded, actual)) in recorded.iter().zip(actual).enumerate() {
                compare_shape(
                    recorded,
                    actual,
                    &format!("{pointer}/{index}"),
                    documented,
                    diffs,
                );
            }
        }
        _ if kind(recorded) != kind(actual) => diffs.push(format!(
            "{}: recorded {} {}, fake {} {}",
            if pointer.is_empty() { "body" } else { pointer },
            kind(recorded),
            excerpt(recorded),
            kind(actual),
            excerpt(actual)
        )),
        _ => {}
    }
}

/// A response from the fake.
struct Reply {
    status: u16,
    headers: reqwest::header::HeaderMap,
    body: Value,
    bytes: Vec<u8>,
}

impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(name).and_then(|value| value.to_str().ok())
    }
}

/// Replays one scenario and collects its differences from the recording.
struct Replay<'a> {
    server: &'a Server,
    client: Client,
    tokens: HashMap<&'static str, String>,
    /// Fake values of recorded ids and shas, keyed by entity (or `sha`) and recorded value.
    ids: HashMap<(String, String), String>,
    replies: Vec<Reply>,
    diffs: Vec<String>,
}

impl<'a> Replay<'a> {
    /// Seeds the fixture accounts: both users, and the organization user A administers.
    fn new(server: &'a Server) -> Self {
        let tokens = HashMap::from([
            (USER_A, server.add_user(USER_A).add_token(SCOPES)),
            (USER_B, server.add_user(USER_B).add_token(SCOPES)),
        ]);
        server.add_org(ORG, &[USER_A]);
        Self {
            server,
            client: Client::builder().redirect(Policy::none()).build().unwrap(),
            tokens,
            ids: HashMap::new(),
            replies: Vec::new(),
            diffs: Vec::new(),
        }
    }

    /// Records that the recording's `recorded` id of `entity` is `actual` in the fake.
    fn known(&mut self, entity: &str, recorded: &str, actual: &str) {
        self.ids
            .insert((entity.to_owned(), recorded.to_owned()), actual.to_owned());
    }

    /// The fake's id of `repository`, known to the recording as id 1000.
    fn known_repository(&mut self, repository: &Repository) {
        let path = format!("/repos/{}/{}", repository.owner(), repository.name());
        let token = self.tokens[USER_A].clone();
        let reply = self.send("GET", &self.url(&path), Some(&token), None, &Value::Null);
        assert_eq!(reply.status, 200, "GET {path}");
        let id = reply.body["id"].to_string();
        self.known("repository", "1000", &id);
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.server.url())
    }

    fn send(
        &self,
        method: &str,
        url: &str,
        token: Option<&str>,
        headers: Option<&Map<String, Value>>,
        body: &Value,
    ) -> Reply {
        let mut request = self
            .client
            .request(method.parse().unwrap(), url)
            .header("user-agent", "octokit-fixtures-replay");
        if let Some(token) = token {
            request = request.header("authorization", format!("token {token}"));
        }
        for name in ["accept", "content-type"] {
            if let Some(value) = headers.and_then(|headers| headers.get(name)) {
                request = request.header(name, value.as_str().unwrap());
            }
        }
        request = match body {
            Value::String(text) if text.is_empty() => request,
            Value::String(text) => request.body(text.clone()),
            Value::Null => request,
            body => request
                .header("content-type", "application/json; charset=utf-8")
                .body(body.to_string()),
        };
        if matches!(method, "PUT" | "PATCH") && body.as_str() == Some("") {
            request = request.header("content-length", "0");
        }
        let response = request.send().unwrap();
        let status = response.status().as_u16();
        let headers = response.headers().clone();
        let bytes = response.bytes().unwrap().to_vec();
        let body = if bytes.is_empty() {
            Value::String(String::new())
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        Reply {
            status,
            headers,
            body,
            bytes,
        }
    }

    /// `path` with its recorded ids and shas replaced by the fake's.
    fn translate_path(&self, path: &str) -> String {
        let (path, query) = match path.split_once('?') {
            Some((path, query)) => (path, Some(query)),
            None => (path, None),
        };
        let segments: Vec<&str> = path.split('/').collect();
        let mut translated = Vec::with_capacity(segments.len());
        for (index, segment) in segments.iter().enumerate() {
            let entity = index
                .checked_sub(1)
                .and_then(|previous| {
                    PATH_IDS
                        .iter()
                        .find(|(name, _)| *name == segments[previous])
                })
                .map(|(_, entity)| *entity);
            let actual =
                entity.and_then(|entity| self.ids.get(&(entity.to_owned(), segment.to_string())));
            translated.push(actual.map_or(*segment, String::as_str));
        }
        let path = translated.join("/");
        match query {
            Some(query) => format!("{path}?{query}"),
            None => path,
        }
    }

    /// `body` with recorded shas replaced by the fake's.
    fn translate_body(&self, body: &Value) -> Value {
        match body {
            Value::String(text) if is_placeholder_sha(text) => self
                .ids
                .get(&("sha".to_owned(), text.clone()))
                .map_or_else(|| body.clone(), |sha| Value::String(sha.clone())),
            Value::Object(object) => Value::Object(
                object
                    .iter()
                    .map(|(key, value)| (key.clone(), self.translate_body(value)))
                    .collect(),
            ),
            Value::Array(items) => {
                Value::Array(items.iter().map(|item| self.translate_body(item)).collect())
            }
            other => other.clone(),
        }
    }

    /// Learns the fake's ids and shas from a response paired with its recording.
    fn learn(&mut self, recorded: &Value, actual: &Value, pointer: &str) {
        match (recorded, actual) {
            (Value::Object(recorded), Value::Object(actual)) => {
                if let (Some(entity), Some(Value::Number(id)), Some(Value::Number(other))) =
                    (entity(recorded), recorded.get("id"), actual.get("id"))
                {
                    let key = (entity.to_owned(), id.to_string());
                    match self.ids.get(&key) {
                        Some(known) if *known != other.to_string() => self.diffs.push(format!(
                            "{pointer}/id: recorded {entity} {id} is {known} in the fake, \
                             but this response has {other}"
                        )),
                        Some(_) => {}
                        None => {
                            self.ids.insert(key, other.to_string());
                        }
                    }
                }
                if let (Some(Value::String(sha)), Some(Value::String(other))) =
                    (recorded.get("sha"), actual.get("sha"))
                    && is_placeholder_sha(sha)
                {
                    self.ids
                        .entry(("sha".to_owned(), sha.clone()))
                        .or_insert_with(|| other.clone());
                }
                for (key, value) in recorded {
                    if let Some(other) = actual.get(key) {
                        self.learn(value, other, &format!("{pointer}/{key}"));
                    }
                }
            }
            (Value::Array(recorded), Value::Array(actual)) => {
                for (index, (recorded, actual)) in recorded.iter().zip(actual).enumerate() {
                    self.learn(recorded, actual, &format!("{pointer}/{index}"));
                }
            }
            _ => {}
        }
    }

    /// Sends every recorded request of `scenario` and compares the responses.
    fn run(&mut self, scenario: &str) {
        self.run_with(scenario, usize::MAX, |_| {});
    }

    /// Like [`Self::run`], running the unrecorded `setup` before the exchange numbered `at`.
    fn run_with(&mut self, scenario: &str, at: usize, setup: impl FnOnce(&mut Self)) {
        let mut setup = Some(setup);
        let mut location: Option<String> = None;
        for (index, exchange) in load(scenario).into_iter().enumerate() {
            if index == at {
                setup.take().unwrap()(self);
            }
            let path = self.translate_path(&exchange.path);
            let operation = operation(&exchange.method, &exchange.path);
            let label = format!(
                "#{index} {} {} ({})",
                exchange.method,
                exchange.path,
                match operation {
                    Some(info) if IMPLEMENTED.contains(&info) => info.id.to_owned(),
                    Some(info) => format!("not implemented: {}", info.id),
                    None => "not in the spec".to_owned(),
                }
            );
            let url = match exchange.scope.as_str() {
                "https://api.github.com:443" => self.url(&path),
                "https://uploads.github.com:443" => self.url(&format!("/api/uploads{path}")),
                // The archive download follows the redirect the previous response returned.
                "https://codeload.github.com:443" => match location.take() {
                    Some(location) => location,
                    None => {
                        self.diffs
                            .push(format!("{label}: no redirect from the previous request"));
                        continue;
                    }
                },
                scope => panic!("unexpected scope {scope}"),
            };
            let body = self.translate_body(&exchange.body);
            let token = exchange
                .account
                .as_deref()
                .map(|account| self.tokens[account].clone());
            let reply = self.send(
                &exchange.method,
                &url,
                token.as_deref(),
                Some(&exchange.request_headers),
                &body,
            );
            let before = self.diffs.len();
            if reply.status != exchange.status {
                self.diffs.push(format!(
                    "status: recorded {}, fake {} {}",
                    exchange.status,
                    reply.status,
                    excerpt(&reply.body)
                ));
            } else {
                let mut diffs = Vec::new();
                let documented = operation.map(|info| documented_keys(info, reply.status));
                compare_shape(
                    &exchange.response,
                    &reply.body,
                    "",
                    documented.as_ref(),
                    &mut diffs,
                );
                self.diffs.extend(diffs);
                self.learn(&exchange.response, &reply.body, "");
                if let Some(recorded) = exchange.response_headers.get("location") {
                    let recorded = recorded.as_str().unwrap();
                    let expected = recorded
                        .split_once("://")
                        .and_then(|(_, rest)| rest.split_once('/'))
                        .map(|(_, path)| self.translate_path(&format!("/{path}")))
                        .unwrap();
                    match reply.header("location") {
                        Some(actual) if actual.ends_with(&expected) => {}
                        actual => self
                            .diffs
                            .push(format!("location: recorded {recorded}, fake {actual:?}")),
                    }
                }
            }
            for diff in &mut self.diffs[before..] {
                *diff = format!("{label}: {diff}");
            }
            location = reply.header("location").map(str::to_owned);
            self.replies.push(reply);
        }
    }

    /// Records a difference unless `holds`, for the values a scenario's `test.js` asserts.
    fn expect(&mut self, holds: bool, assertion: &str) {
        if !holds {
            self.diffs.push(format!("test.js: {assertion}"));
        }
    }

    /// Fails with every difference from the recording.
    fn finish(self) {
        assert!(
            self.diffs.is_empty(),
            "{} differences from the recording:\n{}",
            self.diffs.len(),
            self.diffs.join("\n")
        );
    }
}

/// Creates an empty public repository, as `temporary-repository.js` does for most scenarios.
fn temporary_repository(server: &Server, name: &str) -> Repository {
    server
        .add_repository(ORG, name, false, None, None, "main")
        .unwrap()
}

fn commit_readme(repository: &Repository, content: &str) -> String {
    repository
        .commit_file(
            "main",
            Path::new("README.md"),
            content.as_bytes(),
            "initial commit",
        )
        .unwrap()
        .sha()
        .to_owned()
}

fn issue(title: &str, body: Option<&str>) -> ninot_github::fixtures::IssueSeed {
    use ninot_github::fixtures::SeedField;
    ninot_github::fixtures::IssueSeed {
        title: SeedField::present(title.to_owned()),
        body: SeedField::present(body.map(str::to_owned)),
        ..Default::default()
    }
}

/// `octokit-fixture-org/hello-world`, the long-lived repository of the read-only scenarios.
fn hello_world(server: &Server) -> Repository {
    let repository = server
        .add_repository(ORG, "hello-world", false, None, None, "master")
        .unwrap();
    repository
        .commit_file(
            "master",
            Path::new("README.md"),
            b"# hello-world",
            "initial commit",
        )
        .unwrap();
    repository
}

/// A temporary repository with a README commit and the release `v1.0.0` on it. The fake has no
/// release seed, so the release is created through the API as `record.js` does.
fn release_repository(replay: &mut Replay, name: &str) -> String {
    let repository = temporary_repository(replay.server, name);
    let sha = commit_readme(&repository, &format!("# {name}"));
    replay.known_repository(&repository);
    let token = replay.tokens[USER_A].clone();
    let release = replay.send(
        "POST",
        &replay.url(&format!("/repos/{ORG}/{name}/releases")),
        Some(&token),
        None,
        &json!({
            "tag_name": "v1.0.0",
            "name": "Version 1.0.0",
            "body": "Initial release",
            "target_commitish": sha,
        }),
    );
    assert_eq!(release.status, 201, "create release: {}", release.body);
    let id = release.body["id"].to_string();
    replay.known("release", "1000", &id);
    id
}

#[test]
#[ignore = "not implemented: repos/add-collaborator, repos/list-invitations, repos/accept-invitation-for-authenticated-user, repos/list-collaborators, repos/remove-collaborator"]
fn add_and_remove_repository_collaborator() {
    let server = Server::start().unwrap();
    let mut replay = Replay::new(&server);
    let repository = temporary_repository(&server, "add-and-remove-repository-collaborator");
    replay.known_repository(&repository);
    replay.run("add-and-remove-repository-collaborator");
    let invited = replay.replies[0].body["id"].clone();
    let listed = replay.replies[1].body[0]["id"].clone();
    replay.expect(
        invited.is_number() && listed == invited,
        "invitations[0].id is the id of the invitation (1000)",
    );
    replay.finish();
}

#[test]
#[ignore = "fake bug: created issue lacks body, assignees, reactions, timeline_url, state_reason, active_lock_reason, closed_by, performed_via_github_app and the Location header; added labels have description \"\" instead of null"]
fn add_labels_to_issue() {
    let server = Server::start().unwrap();
    let mut replay = Replay::new(&server);
    let repository = temporary_repository(&server, "add-labels-to-issue");
    replay.known_repository(&repository);
    replay.run("add-labels-to-issue");
    replay.finish();
}

#[test]
#[ignore = "not implemented: repos/get-branch-protection, repos/update-branch-protection, repos/delete-branch-protection"]
fn branch_protection() {
    let server = Server::start().unwrap();
    let mut replay = Replay::new(&server);
    let repository = temporary_repository(&server, "branch-protection");
    commit_readme(&repository, "# branch-protection");
    server
        .org(ORG)
        .unwrap()
        .add_team("a-team", &[USER_A])
        .unwrap();
    replay.known_repository(&repository);
    replay.run("branch-protection");
    replay.finish();
}

#[test]
#[ignore = "fake bug: commit lacks parents, verification.payload and verification.signature"]
fn create_file() {
    let server = Server::start().unwrap();
    let mut replay = Replay::new(&server);
    let repository = temporary_repository(&server, "create-file");
    replay.known_repository(&repository);
    replay.run("create-file");
    let file_type = replay.replies[0].body["content"]["type"].clone();
    replay.expect(file_type == "file", "content.type is \"file\"");
    replay.finish();
}

#[test]
#[ignore = "not implemented: repos/create-commit-status"]
fn create_status() {
    let server = Server::start().unwrap();
    let mut replay = Replay::new(&server);
    let repository = temporary_repository(&server, "create-status");
    let sha = commit_readme(&repository, "# create-status");
    replay.known_repository(&repository);
    replay.known("sha", "0000000000000000000000000000000000000001", &sha);
    replay.run("create-status");
    let state = replay.replies[3].body["state"].clone();
    replay.expect(state == "failure", "combined status state is \"failure\"");
    replay.finish();
}

#[test]
#[ignore = "fake bug: 422 body has an undocumented status field"]
fn errors() {
    let server = Server::start().unwrap();
    let mut replay = Replay::new(&server);
    let repository = temporary_repository(&server, "errors");
    replay.known_repository(&repository);
    replay.run("errors");
    let status = replay.replies[0].status;
    replay.expect(status == 422, "status is 422");
    replay.finish();
}

#[test]
#[ignore = "not implemented: repos/download-tarball-archive"]
fn get_archive() {
    let server = Server::start().unwrap();
    let mut replay = Replay::new(&server);
    let repository = temporary_repository(&server, "get-archive");
    commit_readme(&repository, "# get-archive");
    replay.known_repository(&repository);
    replay.run("get-archive");
    let redirect = replay.replies[0].status;
    replay.expect(redirect == 302, "tarball status is 302");
    let size = replay.replies.get(1).map(|reply| reply.bytes.len());
    replay.expect(size == Some(166), "archive is 166 bytes (332 hex digits)");
    replay.finish();
}

#[test]
#[ignore = "fake bug: contents/ with an empty path is 404, and the raw media type returns JSON instead of the file"]
fn get_content() {
    let server = Server::start().unwrap();
    let mut replay = Replay::new(&server);
    let repository = hello_world(&server);
    replay.known_repository(&repository);
    replay.run("get-content");
    let listing = replay.replies[0].body.clone();
    replay.expect(
        listing.as_array().map(Vec::len) == Some(1),
        "directory listing has 1 entry",
    );
    replay.expect(listing[0]["path"] == "README.md", "entry path is README.md");
    let raw = &replay.replies[1];
    let (raw_body, content_type) = (
        String::from_utf8_lossy(&raw.bytes).into_owned(),
        raw.header("content-type").unwrap_or("").to_owned(),
    );
    replay.expect(
        raw_body == "# hello-world",
        "raw content is \"# hello-world\"",
    );
    replay.expect(
        content_type == "application/vnd.github.v3.raw; charset=utf-8",
        "raw content-type is \"application/vnd.github.v3.raw; charset=utf-8\"",
    );
    replay.finish();
}

#[test]
#[ignore = "fake bug: description is \"\" instead of null, and 18 documented fields such as plan, disk_usage and members_can_create_repositories are missing"]
fn get_organization() {
    let server = Server::start().unwrap();
    let mut replay = Replay::new(&server);
    // The fake shows these settings only once set; the fixture organization has both. There is
    // no organization profile seed, so they are set through the API.
    let token = replay.tokens[USER_A].clone();
    let update = replay.send(
        "PATCH",
        &replay.url(&format!("/orgs/{ORG}")),
        Some(&token),
        None,
        &json!({
            "billing_email": "billing@example.com",
            "default_repository_permission": "read",
        }),
    );
    assert_eq!(update.status, 200, "update organization: {}", update.body);
    replay.run("get-organization");
    let id = replay.replies[0].body["id"].clone();
    replay.expect(id.is_number(), "id is the organization's id (1000)");
    replay.finish();
}

#[test]
#[ignore = "fake bug: organization, topics and temp_clone_token are missing"]
fn get_repository() {
    let server = Server::start().unwrap();
    let mut replay = Replay::new(&server);
    let repository = hello_world(&server);
    replay.known_repository(&repository);
    replay.run("get-repository");
    let id = replay.replies[0].body["id"].to_string();
    let expected = replay.ids[&("repository".to_owned(), "1000".to_owned())].clone();
    replay.expect(id == expected, "id is the repository's id (1000)");
    replay.finish();
}

#[test]
#[ignore = "fake bug: hub_url and topic_search_url are missing"]
fn get_root() {
    let server = Server::start().unwrap();
    let mut replay = Replay::new(&server);
    replay.run("get-root");
    replay.finish();
}

#[test]
#[ignore = "not implemented: undocumented GET /repos/{owner}/{repo}/git/refs/"]
fn git_refs() {
    let server = Server::start().unwrap();
    let mut replay = Replay::new(&server);
    let repository = temporary_repository(&server, "git-refs");
    let first = repository
        .commit_file(
            "main",
            Path::new("1.md"),
            b"# git-refs\ncreated with initial commit",
            "initial commit",
        )
        .unwrap();
    let second = repository
        .commit_file(
            "main",
            Path::new("2.md"),
            b"# git-refs\ncreated with 2nd commit",
            "2nd commit",
        )
        .unwrap();
    replay.known_repository(&repository);
    // The recording first lists main, at the 2nd commit, then creates a ref at the initial one.
    replay.known(
        "sha",
        "0000000000000000000000000000000000000001",
        second.sha(),
    );
    replay.known(
        "sha",
        "0000000000000000000000000000000000000002",
        first.sha(),
    );
    replay.run("git-refs");
    replay.finish();
}

#[test]
#[ignore = "fake bug: a new repository has no default labels (recorded 9)"]
fn labels() {
    let server = Server::start().unwrap();
    let mut replay = Replay::new(&server);
    let repository = temporary_repository(&server, "labels");
    replay.known_repository(&repository);
    replay.run("labels");
    replay.finish();
}

#[test]
fn lock_issue() {
    let server = Server::start().unwrap();
    let mut replay = Replay::new(&server);
    let repository = temporary_repository(&server, "lock-issue");
    repository
        .add_issue(1, issue("Issue without a label", None))
        .unwrap();
    replay.known_repository(&repository);
    replay.run("lock-issue");
    replay.finish();
}

#[test]
#[ignore = "not implemented: markdown/render, markdown/render-raw"]
fn markdown() {
    let server = Server::start().unwrap();
    let mut replay = Replay::new(&server);
    let repository = hello_world(&server);
    replay.known_repository(&repository);
    replay.run("markdown");
    let recorded = load("markdown");
    for (index, exchange) in recorded.iter().enumerate() {
        let actual = replay
            .replies
            .get(index)
            .map(|reply| String::from_utf8_lossy(&reply.bytes).into_owned());
        replay.expect(
            actual.as_deref() == exchange.response.as_str(),
            &format!("{} renders the recorded HTML", exchange.path),
        );
    }
    replay.finish();
}

#[test]
#[ignore = "not implemented: activity/mark-notifications-as-read"]
fn mark_notifications_as_read() {
    let server = Server::start().unwrap();
    let mut replay = Replay::new(&server);
    replay.run("mark-notifications-as-read");
    replay.finish();
}

#[test]
#[ignore = "not implemented: undocumented GET /repositories/{id}/issues"]
fn paginate_issues() {
    let server = Server::start().unwrap();
    let mut replay = Replay::new(&server);
    let repository = temporary_repository(&server, "paginate-issues");
    for number in 1..=13 {
        repository
            .add_issue(number, issue(&format!("Test issue {number}"), None))
            .unwrap();
    }
    replay.known_repository(&repository);
    replay.run("paginate-issues");
    replay.finish();
}

#[test]
#[ignore = "not implemented: classic project cards, no longer in the spec"]
fn project_cards() {
    let server = Server::start().unwrap();
    let mut replay = Replay::new(&server);
    let repository = temporary_repository(&server, "project-cards");
    replay.known_repository(&repository);
    replay.run("project-cards");
    replay.finish();
}

#[test]
#[ignore = "not implemented: repos/update-release-asset"]
fn release_assets() {
    let server = Server::start().unwrap();
    let mut replay = Replay::new(&server);
    release_repository(&mut replay, "release-assets");
    replay.run("release-assets");
    replay.finish();
}

#[test]
#[ignore = "fake bug: 422 body has an undocumented status field"]
fn release_assets_conflict() {
    let server = Server::start().unwrap();
    let mut replay = Replay::new(&server);
    let release = release_repository(&mut replay, "release-assets-conflict");
    // record.js uploads the asset once unrecorded after reading the release, so the recorded
    // upload conflicts.
    replay.run_with("release-assets-conflict", 1, |replay| {
        let token = replay.tokens[USER_A].clone();
        let mut headers = Map::new();
        headers.insert("content-type".to_owned(), json!("text/plain"));
        let upload = replay.send(
            "POST",
            &replay.url(&format!(
                "/api/uploads/repos/{ORG}/release-assets-conflict/releases/{release}/assets\
                 ?name=test-upload.txt&label=test"
            )),
            Some(&token),
            Some(&headers),
            &json!("Hello, world!\n"),
        );
        assert_eq!(upload.status, 201, "upload asset: {}", upload.body);
    });
    let conflict = replay.replies[1].status;
    replay.expect(conflict == 422, "second upload status is 422");
    replay.finish();
}

#[test]
#[ignore = "not implemented: undocumented PATCH /repositories/{id}"]
fn rename_repository() {
    let server = Server::start().unwrap();
    let mut replay = Replay::new(&server);
    let repository = temporary_repository(&server, "rename-repository");
    replay.known_repository(&repository);
    replay.run("rename-repository");
    replay.finish();
}

#[test]
#[ignore = "fake bug: items lack reactions, timeline_url, state_reason, active_lock_reason and performed_via_github_app"]
fn search_issues() {
    let server = Server::start().unwrap();
    let mut replay = Replay::new(&server);
    let repository = temporary_repository(&server, "search-issues");
    repository
        .add_issue(
            1,
            issue(
                "The doors don’t open",
                Some("I tried \"open sesame\" as seen on Wikipedia but no luck!"),
            ),
        )
        .unwrap();
    repository
        .add_issue(
            2,
            issue(
                "Sesame seeds split without a pop!",
                Some("I’ve waited all year long, but there was no pop 😭"),
            ),
        )
        .unwrap();
    replay.known_repository(&repository);
    replay.run("search-issues");
    replay.finish();
}
