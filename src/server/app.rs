//! The request pipeline: dispatch, authentication, routing and the HTTP response.

use super::*;

/// The response for a request body that could not be read: `413` when it exceeded
/// [`MAX_BODY_BYTES`], otherwise `400`.
fn body_error(error: &axum::Error) -> ApiResponse {
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        if cause.is::<http_body_util::LengthLimitError>() {
            return body_too_large();
        }
        source = cause.source();
    }
    ApiResponse::error(400, "cannot read request body")
}

fn body_too_large() -> ApiResponse {
    ApiResponse::error(413, "Request body too large")
}

/// The largest request body the fake reads, 100 MiB (GitHub's hard limit for one file). A
/// larger body is answered with `413`, from its `Content-Length` before any of it is read, or
/// once a chunked body passes the limit.
const MAX_BODY_BYTES: usize = 100 * 1024 * 1024;

pub(super) async fn dispatch(AxumState(app): AxumState<Arc<App>>, request: Request) -> Response {
    let (parts, body) = request.into_parts();
    let declared = parts
        .headers
        .get(http::header::CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<usize>().ok());
    if declared.is_some_and(|length| length > MAX_BODY_BYTES) {
        return into_response(body_too_large(), None);
    }
    let body = match to_bytes(body, MAX_BODY_BYTES).await {
        Ok(body) => body,
        Err(error) => return into_response(body_error(&error), None),
    };
    let path = parts.uri.path().to_owned();
    let handler_app = app.clone();
    let response = tokio::task::spawn_blocking(move || handler_app.handle(parts, body))
        .await
        .unwrap_or_else(|_| {
            app.record(Hit::Panic { path });
            into_response(ApiResponse::internal("handler panicked"), None)
        });
    let fault = response.extensions().get::<Fault>().cloned();
    match fault {
        Some(fault) => ninot_common::faults::apply(&fault, response).await,
        None => response,
    }
}

/// Body of a `403` secondary rate limit, as GitHub words it.
const SECONDARY_RATE_LIMIT: &str = "You have exceeded a secondary rate limit. Please wait a few minutes before you try again. If you reach out to GitHub Support for help, please include the request ID.";

impl App {
    /// Applies rate limits and injected faults around [`App::serve`].
    fn handle(&self, parts: Parts, body: bytes::Bytes) -> Response {
        let full_path = parts.uri.path();
        let path = full_path
            .strip_prefix(ENTERPRISE_PREFIX)
            .or_else(|| full_path.strip_prefix(UPLOADS_PREFIX))
            .filter(|rest| rest.is_empty() || rest.starts_with('/'))
            .map_or(full_path, |rest| if rest.is_empty() { "/" } else { rest })
            .to_owned();
        let git_path = git::parse_path(full_path);
        let git_service = git_path.as_ref().map(|git_path| {
            if git::is_push(git_path.service_path, parts.uri.query().unwrap_or("")) {
                "receive-pack"
            } else {
                "upload-pack"
            }
        });
        let operation = match git_path {
            Some(_) => None,
            None => method(&parts.method)
                .and_then(|method| self.router.find(method, &path))
                .map(|found| found.info.id),
        };
        let graphql = parts.method == http::Method::POST
            && (full_path == "/api/graphql" || path == "/graphql");
        let client = self.client(&parts);
        let resource = match operation {
            // Source: <https://docs.github.com/en/rest/rate-limit/rate-limit#get-rate-limit-status-for-the-authenticated-user>
            Some("rate-limit/get") => None,
            // Source: <https://docs.github.com/en/rest/search/search#rate-limit>
            Some(id) if id.starts_with("search/") => Some(Resource::Search),
            Some(_) => Some(Resource::Core),
            // GraphQL answers anonymous callers with a 401 before running anything.
            None if graphql && matches!(client, Client::Token(_)) => Some(Resource::Graphql),
            None => None,
        };
        let creates_content =
            !graphql && !matches!(parts.method, http::Method::GET | http::Method::HEAD);
        let admitted = match resource {
            None => None,
            Some(resource) => {
                let admitted =
                    lock(&self.state)
                        .rate_limits
                        .admit(&client, resource, creates_content);
                match admitted {
                    Ok(window) => Some((resource, window)),
                    Err(refusal) => return self.refuse(&client, resource, refusal),
                }
            }
        };
        let mut response = self.apply_faults(parts, body, &path, operation, git_service, &client);
        let mut state = lock(&self.state);
        let headers = match admitted {
            Some((resource, window)) => {
                state.rate_limits.release(&client);
                window.headers(resource)
            }
            None if operation == Some("rate-limit/get") => state
                .rate_limits
                .window(&client, Resource::Core)
                .headers(Resource::Core),
            None => Vec::new(),
        };
        for (name, value) in headers {
            response.headers_mut().insert(
                http::HeaderName::from_bytes(name.as_bytes()).unwrap(),
                HeaderValue::from_str(&value).unwrap(),
            );
        }
        if let Some(operation) = operation {
            response
                .extensions_mut()
                .insert(Label(operation.to_owned()));
        }
        response
    }

    /// Answers a request refused by the rate limits.
    // Source: <https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api#exceeding-the-rate-limit>
    // Source: cli/cli v2.101.0 pkg/cmd/skills/search/search.go:704
    fn refuse(&self, client: &Client, resource: Resource, refusal: Refusal) -> Response {
        let (mut response, window) = match refusal {
            Refusal::Primary(window) => {
                let state = lock(&self.state);
                let who = match client {
                    Client::Ip(ip) => ip.to_string(),
                    Client::Token(token) => {
                        let login = &state.tokens[token].login;
                        let id = state.account(login).map_or(0, |account| account.id);
                        format!("user ID {id}")
                    }
                };
                let message = format!("API rate limit exceeded for {who}.");
                (ApiResponse::error(403, &message), window)
            }
            Refusal::Secondary {
                window,
                retry_after,
            } => (
                ApiResponse::error(403, SECONDARY_RATE_LIMIT)
                    .with_header("retry-after", retry_after.to_string()),
                window,
            ),
        };
        response.headers.extend(window.headers(resource));
        into_response(response, None)
    }

    /// Who rate limits count a request against: its valid token, or else its peer address.
    // Source: <https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api#primary-rate-limit-for-unauthenticated-users>
    fn client(&self, parts: &Parts) -> Client {
        let token = token_from_authorization(&parts.headers)
            .filter(|token| lock(&self.state).tokens.contains_key(token));
        match token {
            Some(token) => Client::Token(token),
            None => {
                let ConnectInfo(address) = parts
                    .extensions
                    .get::<ConnectInfo<SocketAddr>>()
                    .expect("connections carry their peer address");
                Client::Ip(address.ip())
            }
        }
    }

    /// Applies injected faults around [`App::serve`].
    fn apply_faults(
        &self,
        parts: Parts,
        body: bytes::Bytes,
        path: &str,
        operation: Option<&str>,
        git_service: Option<&str>,
        client: &Client,
    ) -> Response {
        let info = RequestInfo {
            method: parts.method.as_str(),
            path,
            operation,
            git_service,
        };
        let decision = lock(&self.state)
            .faults
            .decide(info.method, info.path, |target| target.matches(&info));
        if !decision.latency.is_zero() {
            std::thread::sleep(decision.latency);
        }
        let terminal = match decision.terminal {
            None => return self.serve(parts, body, client),
            Some(Fault::Status(status)) => {
                let message = if status >= 500 {
                    "Server Error"
                } else {
                    "Injected fault"
                };
                return into_response(ApiResponse::error(status, message), None);
            }
            Some(Fault::SecondaryRateLimit) => {
                return into_response(
                    ApiResponse::error(403, SECONDARY_RATE_LIMIT)
                        .with_header("retry-after", "60".to_owned()),
                    None,
                );
            }
            Some(terminal) => terminal,
        };
        let mut response = match terminal {
            Fault::Drop => Response::new(AxumBody::empty()),
            _ => self.serve(parts, body, client),
        };
        response.extensions_mut().insert(terminal);
        response
    }

    fn serve(&self, parts: Parts, body: bytes::Bytes, client: &Client) -> Response {
        let host = parts
            .headers
            .get(http::header::HOST)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("localhost")
            .to_owned();
        let web_url = format!("{}://{host}", self.scheme);
        let full_path = parts.uri.path();
        if parts.method == http::Method::GET
            && let Some(rest) = full_path.strip_prefix("/_ninot/actions/logs/")
        {
            let parts: Vec<&str> = rest.split('/').collect();
            if let [owner, repo, id] = parts.as_slice()
                && let Ok(id) = id.parse::<u64>()
            {
                let text = lock(&self.state)
                    .account(owner)
                    .and_then(|account| account.repo(repo))
                    .and_then(|repo| repo.workflow_job_logs.get(&id))
                    .cloned();
                if let Some(text) = text {
                    return into_response(
                        ApiResponse::bytes(200, "text/plain; charset=utf-8", text.into_bytes()),
                        None,
                    );
                }
            }
            return into_response(ApiResponse::not_found(), None);
        }
        if parts.method == http::Method::GET
            && let Some(rest) = full_path.strip_prefix("/_ninot/actions/log-archives/")
        {
            let segments: Vec<&str> = rest.split('/').collect();
            if let [owner, repo_name, run_id] = segments.as_slice()
                && let Ok(run_id) = run_id.parse::<u64>()
            {
                let caller = match self.authenticate(&parts.headers) {
                    Ok(caller) => caller,
                    Err(response) => return into_response(response, None),
                };
                let archive_data = {
                    let state = lock(&self.state);
                    if state
                        .permission(caller.as_ref(), owner, repo_name)
                        .is_none()
                    {
                        return into_response(ApiResponse::not_found(), caller.as_ref());
                    }
                    let Some(repository) = state
                        .account(owner)
                        .and_then(|account| account.repo(repo_name))
                    else {
                        return into_response(ApiResponse::not_found(), caller.as_ref());
                    };
                    let run_exists = repository.workflow_runs.iter().any(|run| run.id == run_id);
                    let Some(jobs) = repository.workflow_jobs.get(&run_id) else {
                        return into_response(ApiResponse::not_found(), caller.as_ref());
                    };
                    if !run_exists {
                        return into_response(ApiResponse::not_found(), caller.as_ref());
                    }
                    jobs.iter()
                        .filter_map(|job| {
                            repository
                                .workflow_job_logs
                                .get(&job.id)
                                .map(|text| (job.id, text.clone()))
                        })
                        .collect::<Vec<_>>()
                };
                let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
                let options = zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated);
                for (job_id, log) in archive_data {
                    let path = format!("run-{run_id}/job-{job_id}.txt");
                    if archive.start_file(path, options).is_err()
                        || archive.write_all(log.as_bytes()).is_err()
                    {
                        return into_response(
                            ApiResponse::internal("could not build workflow log archive"),
                            None,
                        );
                    }
                }
                let bytes = match archive.finish() {
                    Ok(bytes) => bytes.into_inner(),
                    Err(_) => {
                        return into_response(
                            ApiResponse::internal("could not build workflow log archive"),
                            None,
                        );
                    }
                };
                return into_response(
                    ApiResponse::bytes(200, "application/zip", bytes),
                    caller.as_ref(),
                );
            }
            return into_response(ApiResponse::not_found(), None);
        }
        let (api_url, path) = match full_path
            .strip_prefix(ENTERPRISE_PREFIX)
            .or_else(|| full_path.strip_prefix(UPLOADS_PREFIX))
        {
            Some(rest) if rest.is_empty() || rest.starts_with('/') => (
                crate::urls::enterprise_api(&web_url),
                if rest.is_empty() { "/" } else { rest },
            ),
            _ => (web_url.clone(), full_path),
        };
        if let Some(response) = handlers::actions_service::serve(
            &self.state,
            &parts.method,
            &parts.headers,
            path,
            parts.uri.query(),
            &body,
            &web_url,
        ) {
            if response.status == 404 {
                self.record(Hit::Unrouted {
                    method: parts.method.to_string(),
                    path: path.to_owned(),
                });
            }
            return into_response(response, None);
        }
        if let Some(response) = handlers::runner_service::serve(
            &self.state,
            &parts.method,
            &parts.headers,
            path,
            parts.uri.query(),
            &body,
            &web_url,
        ) {
            return into_response(response, None);
        }
        if let Some(git_path) = git::parse_path(full_path) {
            let response = self.serve_git(&parts, &body, &git_path);
            self.record(Hit::Git {
                service: git_path.service_path.to_owned(),
                status: response.status().as_u16(),
            });
            return response;
        }
        let caller = match self.authenticate(&parts.headers) {
            Ok(caller) => caller,
            Err(response) => return into_response(response, None),
        };
        if parts.method == http::Method::POST && (full_path == "/api/graphql" || path == "/graphql")
        {
            let response =
                crate::graphql::execute(&self.state, &self.hits, caller.clone(), &web_url, &body);
            return into_response(response, caller.as_ref());
        }
        if parts.method == http::Method::GET
            && let Some(response) =
                handlers::releases::download(&self.state, caller.as_ref(), full_path)
        {
            return into_response(response, caller.as_ref());
        }
        let Some(method) = method(&parts.method) else {
            return into_response(ApiResponse::not_found(), caller.as_ref());
        };
        let Some(found) = self.router.find(method, path) else {
            self.record(Hit::Unrouted {
                method: parts.method.to_string(),
                path: path.to_owned(),
            });
            return into_response(ApiResponse::not_found(), caller.as_ref());
        };
        let operation = found.info.id;
        let served = |response: Response| {
            self.record(Hit::Rest {
                operation,
                status: response.status().as_u16(),
            });
            response
        };
        let Some(handler) = handlers::handler(found.info) else {
            let response = ApiResponse::error(
                501,
                &format!("Not implemented by ninot-github: {}", found.info.id),
            );
            return served(into_response(response, caller.as_ref()));
        };
        if let Err(refusal) = validate::check_request(found.info, &body) {
            return served(into_response(refusal, caller.as_ref()));
        }
        let request = ApiRequest {
            path: path.to_owned(),
            params: found.params,
            query: parts
                .uri
                .query()
                .map(|query| {
                    url::form_urlencoded::parse(query.as_bytes())
                        .into_owned()
                        .collect()
                })
                .unwrap_or_default(),
            headers: parts.headers,
            body,
            caller,
            client: Some(client.clone()),
            api_url,
            web_url,
        };
        let response = handler(&self.state, &request).unwrap_or_else(|error| error);
        if self.validate_responses
            && let Body::Json(body) = &response.body
            && let Err(mismatch) = validate::check(found.info, response.status, body)
        {
            eprintln!("ninot-github: {mismatch}");
            let message = format!(
                "ninot-github returned a response that does not match the spec: {mismatch}"
            );
            return served(into_response(
                ApiResponse::internal(&message),
                request.caller.as_ref(),
            ));
        }
        #[cfg(feature = "response-log")]
        if (200..300).contains(&response.status)
            && let Body::Json(body) = &response.body
        {
            super::response_log::record(operation, response.status, body);
        }
        served(into_response(response, request.caller.as_ref()))
    }

    fn record(&self, hit: Hit) {
        self.hits.lock().unwrap().insert(hit);
    }

    /// Serves git's smart HTTP protocol, enforcing the repository's visibility and permissions.
    fn serve_git(&self, parts: &Parts, body: &bytes::Bytes, path: &git::GitPath) -> Response {
        let query = parts.uri.query().unwrap_or("");
        let push = git::is_push(path.service_path, query);
        let caller = match self.authenticate(&parts.headers) {
            Ok(caller) => caller,
            Err(_) => {
                return git_text(401, "Bad credentials\n")
                    .with_header("www-authenticate", "Basic realm=\"GitHub\"".to_owned())
                    .into_axum();
            }
        };
        let resolved = {
            let state = lock(&self.state);
            let target = match state.permission(caller.as_ref(), path.owner, path.repo) {
                Some(_) => Some((path.owner.to_owned(), path.repo.to_owned())),
                None => state.redirect(path.owner, path.repo).cloned(),
            };
            target.and_then(|(owner, name)| {
                let permission = state.permission(caller.as_ref(), &owner, &name)?;
                let git_dir = state.account(&owner)?.repo(&name)?.git_dir.clone()?;
                Some((owner, name, permission, git_dir))
            })
        };
        let denied = |status: u16, text: &str| {
            if caller.is_none() {
                git_text(401, "Authentication required\n")
                    .with_header("www-authenticate", "Basic realm=\"GitHub\"".to_owned())
                    .into_axum()
            } else {
                git_text(status, text).into_axum()
            }
        };
        let (owner, name, git_dir) = match resolved {
            Some((owner, name, permission, git_dir)) if !push || permission.push => {
                (owner, name, git_dir)
            }
            Some(_) => return denied(403, "Permission denied\n"),
            None => return denied(404, "Repository not found.\n"),
        };
        let header = |name: &str| {
            parts
                .headers
                .get(name)
                .and_then(|value| value.to_str().ok())
        };
        let request = git::CgiRequest {
            method: parts.method.as_str(),
            service_path: path.service_path,
            query,
            content_type: header("content-type"),
            content_encoding: header("content-encoding"),
            git_protocol: header("git-protocol"),
            remote_user: caller.as_ref().map(|caller| caller.login.as_str()),
            export_all: true,
            body,
        };
        let receive = parts.method == http::Method::POST && path.service_path == "git-receive-pack";
        let before = receive.then(|| ref_snapshot(&git_dir));
        let response = match git::http_backend(&git_dir, &request) {
            Ok(response) => response,
            Err(error) => return git_text(500, &format!("{error}\n")).into_axum(),
        };
        if let Some(before) = before.filter(|_| response.status == 200) {
            let after = ref_snapshot(&git_dir);
            let host = parts
                .headers
                .get(http::header::HOST)
                .and_then(|value| value.to_str().ok())
                .unwrap_or("localhost");
            let urls = ApiRequest::for_urls(&format!("{}://{host}", self.scheme), caller.clone());
            let mut state = lock(&self.state);
            let pusher = caller
                .as_ref()
                .and_then(|caller| state.account(&caller.login))
                .map(|account| crate::render::simple_user(&urls, account));
            if let Some(repo) = state
                .account_mut(&owner)
                .and_then(|account| account.repo_mut(&name))
            {
                repo.pushed_at = Some(crate::state::now());
                if let Some(branch) = first_default_branch(&repo.default_branch, &before, &after)
                    && let Err(error) = repo.set_default_branch(&branch)
                {
                    eprintln!("ninot-github: cannot set the default branch to {branch}: {error}");
                }
            }
            if let Some(pusher) = &pusher {
                let at = crate::time::now_utc();
                let mut moved = Vec::new();
                for (name_of_ref, sha) in &after {
                    let Some(branch) = name_of_ref.strip_prefix("refs/heads/") else {
                        continue;
                    };
                    if before.get(name_of_ref) == Some(sha) {
                        continue;
                    }
                    match crate::handlers::timeline::branch_moved(
                        &mut state, &owner, &name, branch, sha, at, pusher,
                    ) {
                        Ok(items) => moved.extend(items),
                        Err(error) => {
                            eprintln!(
                                "ninot-github: cannot move pull requests on {branch}: {error}"
                            );
                        }
                    }
                }
                let mut ctx = crate::webhooks::events::Ctx::new(&mut state, &urls, &owner, &name);
                for item in &moved {
                    // The push has landed, so a payload that cannot be built only fails its
                    // delivery, which records the error.
                    if let Err(error) = crate::webhooks::events::pull_request::synchronize(
                        &mut ctx,
                        &item.pull,
                        &item.before,
                        &item.after,
                        pusher,
                    ) {
                        eprintln!(
                            "ninot-github: no synchronize payload for #{}: {error}",
                            item.pull.number
                        );
                    }
                }
            }
            drop(state);
            webhooks::enqueue_push(&self.state, &urls, &owner, &name, &git_dir, &before, &after);
            webhooks::events::create::from_push(&self.state, &urls, &owner, &name, &before, &after);
            webhooks::events::delete::from_push(&self.state, &urls, &owner, &name, &before, &after);
        }
        let mut reply = ApiResponse::bytes(response.status, "", response.body);
        reply.headers = response.headers;
        into_response(reply, None)
    }

    /// Resolves the `Authorization` header; an unknown token is a 401.
    fn authenticate(&self, headers: &HeaderMap) -> Result<Option<Caller>, ApiResponse> {
        if !headers.contains_key(http::header::AUTHORIZATION) {
            return Ok(None);
        }
        let state = lock(&self.state);
        let token = token_from_authorization(headers).unwrap_or_default();
        if let Some(caller) = state.tokens.get(&token) {
            return Ok(Some(caller.clone()));
        }
        let is_jwt = headers
            .get(http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|value| {
                let mut parts = value.split('.');
                parts.next().is_some_and(|part| !part.is_empty())
                    && parts.next().is_some_and(|part| !part.is_empty())
                    && parts.next().is_some_and(|part| !part.is_empty())
                    && parts.next().is_none()
            });
        if is_jwt {
            return Ok(Some(Caller {
                login: "github-app-jwt".to_owned(),
                scopes: vec!["app".to_owned()],
            }));
        }
        // Source: <https://docs.github.com/en/rest/authentication/authenticating-to-the-rest-api#failed-login-limit>
        Err(ApiResponse::error(401, "Bad credentials"))
    }
}

fn method(method: &http::Method) -> Option<Method> {
    Some(match *method {
        http::Method::GET => Method::Get,
        http::Method::POST => Method::Post,
        http::Method::PUT => Method::Put,
        http::Method::PATCH => Method::Patch,
        http::Method::DELETE => Method::Delete,
        _ => return None,
    })
}

/// Every ref of a bare repository and the sha it points at.
fn ref_snapshot(git_dir: &std::path::Path) -> std::collections::BTreeMap<String, String> {
    git::refs(git_dir, "refs/")
        .unwrap_or_default()
        .into_iter()
        .map(|found| (found.name, found.sha))
        .collect()
}

/// The default branch a push into a repository without branches establishes: the current
/// default, then `main`, then `master`, then the first pushed branch by name. `None` when the
/// repository already had branches or the push created none.
fn first_default_branch(
    current: &str,
    before: &std::collections::BTreeMap<String, String>,
    after: &std::collections::BTreeMap<String, String>,
) -> Option<String> {
    if before.keys().any(|name| name.starts_with("refs/heads/")) {
        return None;
    }
    let pushed: Vec<&str> = after
        .keys()
        .filter_map(|name| name.strip_prefix("refs/heads/"))
        .collect();
    [current, "main", "master"]
        .into_iter()
        .find(|branch| pushed.contains(branch))
        .or_else(|| pushed.first().copied())
        .map(str::to_owned)
}

fn git_text(status: u16, text: &str) -> ApiResponse {
    ApiResponse::bytes(status, "text/plain", text.as_bytes().to_vec())
}

impl ApiResponse {
    fn into_axum(self) -> Response {
        into_response(self, None)
    }
}

fn into_response(response: ApiResponse, caller: Option<&Caller>) -> Response {
    let mut builder = Response::builder()
        .status(StatusCode::from_u16(response.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR));
    let headers = builder.headers_mut().unwrap();
    // Source: cli/cli v2.101.0 pkg/cmd/auth/shared/oauth_scopes.go:56
    if let Some(caller) = caller {
        headers.insert(
            "x-oauth-scopes",
            HeaderValue::from_str(&caller.scopes.join(", ")).unwrap(),
        );
    }
    let body = match response.body {
        Body::Empty => AxumBody::empty(),
        Body::Json(value) => {
            headers.insert(
                http::header::CONTENT_TYPE,
                HeaderValue::from_static("application/json; charset=utf-8"),
            );
            AxumBody::from(serde_json::to_vec(&value).unwrap())
        }
        Body::Bytes(bytes) => AxumBody::from(bytes),
    };
    for (name, value) in response.headers {
        if let (Ok(name), Ok(value)) = (
            http::HeaderName::from_bytes(name.as_bytes()),
            HeaderValue::from_str(&value),
        ) {
            headers.insert(name, value);
        }
    }
    builder.body(body).unwrap()
}
