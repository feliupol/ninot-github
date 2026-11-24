//! The OAuth web flow of a GitHub App, served at the root as github.com serves it.
//!
//! `GET /login/oauth/authorize` approves the request at once, as the user named by the
//! `login` parameter (the admin user without it), and redirects with a one-time `code`.
//! `POST /login/oauth/access_token` exchanges the code for a user access token.
//! Source: <https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/generating-a-user-access-token-for-a-github-app>

use std::sync::Mutex;

use http::{HeaderMap, Method};
use serde::{Deserialize, Serialize};

use crate::api::ApiResponse;
use crate::state::{AccountKind, OauthGrant, State, lock};

const AUTHORIZE: &str = "/login/oauth/authorize";
const ACCESS_TOKEN: &str = "/login/oauth/access_token";
const ERRORS_URL: &str = "https://docs.github.com/apps/managing-oauth-apps/troubleshooting-oauth-app-access-token-request-errors";

/// Answers a request for one of the OAuth endpoints, or `None` for any other path.
pub(crate) fn serve(
    state: &Mutex<State>,
    method: &Method,
    headers: &HeaderMap,
    path: &str,
    query: Option<&str>,
    body: &[u8],
) -> Option<ApiResponse> {
    match (method.as_str(), path) {
        ("GET", AUTHORIZE) => Some(authorize(state, query.unwrap_or(""))),
        ("POST", ACCESS_TOKEN) => Some(access_token(state, headers, query.unwrap_or(""), body)),
        (_, AUTHORIZE | ACCESS_TOKEN) => Some(ApiResponse::not_found()),
        _ => None,
    }
}

/// The parameters of either endpoint; a later source overrides an earlier one.
#[derive(Default, Deserialize)]
#[serde(default)]
struct Params {
    client_id: Option<String>,
    client_secret: Option<String>,
    code: Option<String>,
    redirect_uri: Option<String>,
    state: Option<String>,
    login: Option<String>,
}

impl Params {
    fn from_form(text: &str) -> Self {
        let mut params = Self::default();
        for (name, value) in url::form_urlencoded::parse(text.as_bytes()) {
            let value = Some(value.into_owned());
            match name.as_ref() {
                "client_id" => params.client_id = value,
                "client_secret" => params.client_secret = value,
                "code" => params.code = value,
                "redirect_uri" => params.redirect_uri = value,
                "state" => params.state = value,
                "login" => params.login = value,
                _ => {}
            }
        }
        params
    }

    fn merge(&mut self, other: Self) {
        self.client_id = other.client_id.or(self.client_id.take());
        self.client_secret = other.client_secret.or(self.client_secret.take());
        self.code = other.code.or(self.code.take());
        self.redirect_uri = other.redirect_uri.or(self.redirect_uri.take());
        self.state = other.state.or(self.state.take());
        self.login = other.login.or(self.login.take());
    }
}

fn authorize(state: &Mutex<State>, query: &str) -> ApiResponse {
    let params = Params::from_form(query);
    let (Some(client_id), Some(redirect_uri)) = (&params.client_id, &params.redirect_uri) else {
        return ApiResponse::unprocessable("client_id and redirect_uri are required");
    };
    let login = params
        .login
        .as_deref()
        .unwrap_or(crate::server::ADMIN_LOGIN);
    let Ok(mut location) = url::Url::parse(redirect_uri) else {
        return ApiResponse::unprocessable("redirect_uri is not a URL");
    };
    let mut state = lock(state);
    let user_exists = state
        .account(login)
        .is_some_and(|account| account.kind == AccountKind::User);
    if !state.oauth_apps.contains_key(client_id) || !user_exists {
        return ApiResponse::not_found();
    }
    let code = ninot_common::token::new_token("");
    state.oauth_codes.insert(
        code.clone(),
        OauthGrant {
            client_id: client_id.clone(),
            login: login.to_owned(),
            redirect_uri: redirect_uri.clone(),
        },
    );
    {
        let mut pairs = location.query_pairs_mut();
        pairs.append_pair("code", &code);
        if let Some(sent) = &params.state {
            pairs.append_pair("state", sent);
        }
    }
    ApiResponse::found(location.into())
}

#[derive(Serialize)]
struct TokenReply {
    access_token: String,
    scope: &'static str,
    token_type: &'static str,
}

#[derive(Serialize)]
struct ErrorReply {
    error: &'static str,
    error_description: &'static str,
    error_uri: String,
}

fn access_token(
    state: &Mutex<State>,
    headers: &HeaderMap,
    query: &str,
    body: &[u8],
) -> ApiResponse {
    let json = headers
        .get(http::header::ACCEPT)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|accept| accept.contains("json"));
    let mut params = Params::from_form(query);
    let body_params = if body.first() == Some(&b'{') {
        serde_json::from_slice(body).unwrap_or_default()
    } else {
        Params::from_form(&String::from_utf8_lossy(body))
    };
    params.merge(body_params);
    let mut state = lock(state);
    let app = params
        .client_id
        .as_ref()
        .and_then(|id| state.oauth_apps.get(id))
        .filter(|app| params.client_secret.as_deref() == Some(app.client_secret.as_str()))
        .cloned();
    let Some(app) = app else {
        return failure(
            json,
            "incorrect_client_credentials",
            "The client_id and/or client_secret passed are incorrect.",
            "incorrect-client-credentials",
        );
    };
    let grant = params
        .code
        .as_ref()
        .and_then(|code| state.oauth_codes.get(code))
        .filter(|grant| Some(&grant.client_id) == params.client_id.as_ref())
        .cloned();
    let Some(grant) = grant else {
        return failure(
            json,
            "bad_verification_code",
            "The code passed is incorrect or expired.",
            "bad-verification-code",
        );
    };
    if params
        .redirect_uri
        .as_ref()
        .is_some_and(|uri| *uri != grant.redirect_uri)
    {
        return failure(
            json,
            "redirect_uri_mismatch",
            "The redirect_uri MUST match the registered callback URL for this application.",
            "redirect-uri-mismatch",
        );
    }
    if let Some(code) = &params.code {
        state.oauth_codes.remove(code);
    }
    let reply = TokenReply {
        access_token: state.add_user_to_server_token(&grant.login, app.app_id),
        scope: "",
        token_type: "bearer",
    };
    if json {
        ApiResponse::ok_typed(&reply)
    } else {
        form(&[
            ("access_token", &reply.access_token),
            ("scope", reply.scope),
            ("token_type", reply.token_type),
        ])
    }
}

/// An error answers `200`, as github.com does, in the format the caller asked for.
fn failure(
    json: bool,
    error: &'static str,
    description: &'static str,
    anchor: &str,
) -> ApiResponse {
    let reply = ErrorReply {
        error,
        error_description: description,
        error_uri: format!("{ERRORS_URL}/#{anchor}"),
    };
    if json {
        ApiResponse::ok_typed(&reply)
    } else {
        form(&[
            ("error", reply.error),
            ("error_description", reply.error_description),
            ("error_uri", &reply.error_uri),
        ])
    }
}

fn form(pairs: &[(&str, &str)]) -> ApiResponse {
    let mut body = url::form_urlencoded::Serializer::new(String::new());
    body.extend_pairs(pairs);
    ApiResponse::bytes_ok(
        "application/x-www-form-urlencoded; charset=utf-8",
        body.finish().into_bytes(),
    )
}
