//! Request and response types shared by all handlers.

#![expect(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "untyped JSON; remove when this module is typed"
)]

use std::collections::HashMap;

use headers::{ETag, HeaderMapExt, IfNoneMatch};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::permissions::Access;
use crate::types;

/// Documentation link GitHub attaches to error bodies.
pub const DOCS_URL: &str = "https://docs.github.com/rest";

/// The caller a request is authenticated as.
#[derive(Clone, Debug)]
pub struct Caller {
    pub login: String,
    pub scopes: Vec<String>,
}

/// Scope prefix naming the repository, as `owner/repo`, of an installation token.
pub const INSTALLATION_SCOPE: &str = "installation:";

/// Scope prefix naming the GitHub App, by id, of a user access token.
pub const USER_TO_SERVER_SCOPE: &str = "user-to-server:";

/// Prefix of GitHub App user access tokens.
pub const USER_TOKEN_PREFIX: &str = "ghu_";

/// Scope prefix of one permission an installation token grants, as `permission:<name>:<access>`.
pub const PERMISSION_SCOPE: &str = "permission:";

impl Caller {
    pub fn has_scope(&self, scope: &str) -> bool {
        self.scopes.iter().any(|granted| granted == scope)
    }

    /// The `owner/repo` of an installation token.
    pub fn installation(&self) -> Option<&str> {
        self.scopes
            .iter()
            .find_map(|scope| scope.strip_prefix(INSTALLATION_SCOPE))
    }

    /// The id of the GitHub App a user access token belongs to.
    pub fn user_to_server_app(&self) -> Option<u64> {
        self.scopes
            .iter()
            .find_map(|scope| scope.strip_prefix(USER_TO_SERVER_SCOPE))
            .and_then(|id| id.parse().ok())
    }

    /// The access an installation token grants to `permission`.
    pub fn permission(&self, permission: &str) -> Option<Access> {
        self.scopes
            .iter()
            .filter_map(|scope| scope.strip_prefix(PERMISSION_SCOPE)?.split_once(':'))
            .filter(|(name, _)| *name == permission)
            .filter_map(|(_, access)| match access {
                "read" => Some(Access::Read),
                "write" => Some(Access::Write),
                "admin" => Some(Access::Admin),
                _ => None,
            })
            .max()
    }
}

/// One API request, already routed to an operation.
pub struct ApiRequest {
    /// Request path without the enterprise prefix, such as `/repos/o/r`.
    pub path: String,
    /// Path parameters by their spec name.
    pub params: HashMap<String, String>,
    pub query: Vec<(String, String)>,
    pub headers: http::HeaderMap,
    pub body: bytes::Bytes,
    pub caller: Option<Caller>,
    /// Who rate limits count this request against.
    pub client: Option<crate::rate_limit::Client>,
    /// Base URL of the API as the client addressed it, such as `http://127.0.0.1:1234/api/v3`.
    pub api_url: String,
    /// Base URL of the server, used for web and git URLs.
    pub web_url: String,
}

impl ApiRequest {
    /// A request carrying only URLs and a caller, for rendering outside an API call.
    pub fn for_urls(web_url: &str, caller: Option<Caller>) -> Self {
        Self {
            path: "/".to_owned(),
            params: HashMap::new(),
            query: Vec::new(),
            headers: http::HeaderMap::new(),
            body: bytes::Bytes::new(),
            caller,
            client: None,
            api_url: web_url.to_owned(),
            web_url: web_url.to_owned(),
        }
    }

    /// Locations of resources under this request's base URLs.
    pub fn urls(&self) -> crate::urls::Urls<'_> {
        crate::urls::Urls::new(&self.api_url, &self.web_url)
    }

    /// A path parameter the route guarantees.
    pub fn param(&self, name: &str) -> &str {
        self.params
            .get(name)
            .unwrap_or_else(|| panic!("route has no parameter {name}"))
    }

    pub fn query(&self, name: &str) -> Option<&str> {
        self.query
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }

    /// Deserializes a JSON body into the model generated from this operation's OpenAPI schema.
    ///
    /// # Errors
    ///
    /// Returns 400 when the body is not valid JSON and 422 when its JSON value does not match
    /// the operation's request model.
    pub fn json_as<T: DeserializeOwned>(&self) -> Result<T, ApiResponse> {
        let body = if self.body.is_empty() {
            b"{}".as_slice()
        } else {
            self.body.as_ref()
        };
        serde_json::from_slice(body).map_err(|error| {
            if !error.is_data() || serde_json::from_slice::<serde::de::IgnoredAny>(body).is_err() {
                // Source: <https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api#problems-parsing-json>
                crate::errors::GithubError::ProblemsParsingJson.to_response()
            } else {
                crate::errors::GithubError::InvalidBody.to_response()
            }
        })
    }

    /// The authenticated caller, or a 401 response.
    pub fn require_caller(&self) -> Result<&Caller, ApiResponse> {
        self.caller
            .as_ref()
            .ok_or_else(|| crate::errors::GithubError::RequiresAuthentication.to_response())
    }

    /// The caller, or a 401; with a token lacking every scope in `any_of`, a 403.
    ///
    /// `any_of` lists the classic token scopes of the operation. An installation token passes on
    /// its own repository: [`crate::permissions::authorize`] has checked its permissions.
    // Source: <https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/scopes-for-oauth-apps#available-scopes>
    pub fn require_scope(&self, any_of: &[&str]) -> Result<&Caller, ApiResponse> {
        let caller = self.require_caller()?;
        let own_repository = caller.installation().is_some_and(|installation| {
            self.params
                .get("owner")
                .zip(self.params.get("repo"))
                .is_some_and(|(owner, repo)| {
                    installation.eq_ignore_ascii_case(&format!("{owner}/{repo}"))
                })
        });
        if own_repository || any_of.iter().any(|scope| caller.has_scope(scope)) {
            Ok(caller)
        } else {
            Err(crate::errors::GithubError::PatForbidden.to_response())
        }
    }

    /// The `Accept` header, or an empty string.
    pub fn accept(&self) -> &str {
        self.headers
            .get(http::header::ACCEPT)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
    }
}

/// Body of an API response.
#[derive(Debug)]
pub enum Body {
    Empty,
    Json(Value),
    Bytes(Vec<u8>),
}

/// One API response.
#[derive(Debug)]
pub struct ApiResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Body,
}

impl ApiResponse {
    pub fn ok(value: Value) -> Self {
        Self::json_status(http::StatusCode::OK, value)
    }

    /// Serve a JSON representation with its content-derived conditional validator.
    ///
    /// # Panics
    ///
    /// If a JSON value cannot serialize or a quoted SHA-256 is not an entity tag.
    pub(crate) fn conditional_json(request: &ApiRequest, value: Value) -> Self {
        let bytes = serde_json::to_vec(&value).expect("JSON values serialize");
        let validator = format!("\"{:x}\"", Sha256::digest(bytes));
        let etag = validator
            .parse::<ETag>()
            .expect("quoted SHA-256 is an ETag");
        let unchanged = request
            .headers
            .typed_get::<IfNoneMatch>()
            .is_some_and(|condition| !condition.precondition_passes(&etag));
        // Source: <https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api#use-conditional-requests>
        let response = if unchanged {
            Self::not_modified()
        } else {
            Self::ok(value)
        };
        response
            .with_header("etag", validator)
            .with_header("vary", "Accept, Authorization".to_owned())
    }

    /// Serializes a response model at the conditional HTTP boundary.
    pub(crate) fn conditional_typed(request: &ApiRequest, value: &impl Serialize) -> Self {
        match serde_json::to_value(value) {
            Ok(value) => Self::conditional_json(request, value),
            Err(_) => Self::internal("Response could not be serialized"),
        }
    }

    fn with_status(status: http::StatusCode, value: &impl serde::Serialize) -> Self {
        Self::json_status(
            status,
            serde_json::to_value(value).expect("response types serialize to JSON"),
        )
    }

    /// `200` with a JSON body serialized from one of the spec's response types.
    pub fn ok_typed(value: &impl serde::Serialize) -> Self {
        Self::with_status(http::StatusCode::OK, value)
    }

    /// `201` with a JSON body serialized from one of the spec's response types.
    pub fn created(value: &impl serde::Serialize) -> Self {
        Self::with_status(http::StatusCode::CREATED, value)
    }

    /// `202` with a JSON body serialized from one of the spec's response types.
    pub fn accepted(value: &impl serde::Serialize) -> Self {
        Self::with_status(http::StatusCode::ACCEPTED, value)
    }

    /// `301` with a JSON body. Callers add the `Location` header.
    pub fn moved_permanently(value: &impl serde::Serialize) -> Self {
        Self::with_status(http::StatusCode::MOVED_PERMANENTLY, value)
    }

    /// `201` with a JSON value.
    pub fn created_json(value: Value) -> Self {
        Self::json_status(http::StatusCode::CREATED, value)
    }

    /// `202` with a JSON value.
    pub fn accepted_json(value: Value) -> Self {
        Self::json_status(http::StatusCode::ACCEPTED, value)
    }

    /// JSON at `status`.
    pub fn json_status(status: http::StatusCode, value: Value) -> Self {
        Self {
            status: status.as_u16(),
            headers: Vec::new(),
            body: Body::Json(value),
        }
    }

    pub fn no_content() -> Self {
        Self {
            status: http::StatusCode::NO_CONTENT.as_u16(),
            headers: Vec::new(),
            body: Body::Empty,
        }
    }

    /// `304` with an empty body.
    pub fn not_modified() -> Self {
        Self {
            status: http::StatusCode::NOT_MODIFIED.as_u16(),
            headers: Vec::new(),
            body: Body::Empty,
        }
    }

    /// `302` with an empty body and a `Location` header.
    pub fn found(location: String) -> Self {
        Self {
            status: http::StatusCode::FOUND.as_u16(),
            headers: vec![("location".to_owned(), location)],
            body: Body::Empty,
        }
    }

    /// `303` with an empty body.
    pub fn see_other() -> Self {
        Self {
            status: http::StatusCode::SEE_OTHER.as_u16(),
            headers: Vec::new(),
            body: Body::Empty,
        }
    }

    /// `200` with a raw body.
    pub fn bytes_ok(content_type: &str, bytes: Vec<u8>) -> Self {
        Self::bytes_status(http::StatusCode::OK, content_type, bytes)
    }

    /// `202` with a raw body.
    pub fn bytes_accepted(content_type: &str, bytes: Vec<u8>) -> Self {
        Self::bytes_status(http::StatusCode::ACCEPTED, content_type, bytes)
    }

    /// A raw body at `status`.
    pub fn bytes_status(status: http::StatusCode, content_type: &str, bytes: Vec<u8>) -> Self {
        Self {
            status: status.as_u16(),
            headers: vec![("content-type".to_owned(), content_type.to_owned())],
            body: Body::Bytes(bytes),
        }
    }

    /// A raw body at a status chosen outside this process, such as git's smart HTTP reply.
    pub fn bytes_raw(status: u16, content_type: &str, bytes: Vec<u8>) -> Self {
        Self {
            status,
            headers: vec![("content-type".to_owned(), content_type.to_owned())],
            body: Body::Bytes(bytes),
        }
    }

    /// A GitHub basic-error body. `status` is the numeric code stored in the body.
    pub(crate) fn basic_error_raw(status: u16, documentation_url: &str, message: &str) -> Self {
        Self::json_status(
            http::StatusCode::from_u16(status).unwrap_or(http::StatusCode::INTERNAL_SERVER_ERROR),
            serde_json::to_value(&types::BasicError {
                documentation_url: Some(documentation_url.to_owned()),
                message: Some(message.to_owned()),
                status: Some(status.to_string()),
                url: None,
            })
            .expect("response types serialize to JSON"),
        )
        .with_raw_status(status)
    }

    fn with_raw_status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    /// A 422 validation failure naming the offending field.
    // Source: <https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api#validation-failed>
    pub(crate) fn validation_body(resource: &str, field: &str, code: &str) -> Self {
        let status = http::StatusCode::UNPROCESSABLE_ENTITY;
        Self::with_status(
            status,
            &types::ValidationError {
                documentation_url: DOCS_URL.to_owned(),
                errors: vec![types::ValidationErrorErrorsItem {
                    code: code.to_owned(),
                    field: Some(field.to_owned()),
                    index: None,
                    message: None,
                    resource: Some(resource.to_owned()),
                    value: None,
                }],
                message: "Validation Failed".to_owned(),
                status: status.as_str().to_owned(),
            },
        )
    }

    pub fn not_found() -> Self {
        crate::errors::GithubError::NotFound.to_response()
    }

    /// A 500 whose message is `error`.
    pub(crate) fn internal(error: impl std::fmt::Display) -> Self {
        crate::errors::GithubError::Internal(error.to_string().into()).to_response()
    }

    /// A 422 with a plain message, for failures that are not about one field.
    pub(crate) fn unprocessable(message: &str) -> Self {
        crate::errors::GithubError::Unprocessable(message.to_owned().into()).to_response()
    }

    /// A 422 validation failure naming the offending field.
    // Source: <https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api#validation-failed>
    pub fn validation_failed(resource: &str, field: &str, code: &str) -> Self {
        crate::errors::GithubError::ValidationFailed {
            resource: resource.to_owned().into(),
            field: field.to_owned().into(),
            code: code.to_owned().into(),
        }
        .to_response()
    }

    pub fn with_header(mut self, name: &str, value: String) -> Self {
        self.headers.push((name.to_owned(), value));
        self
    }
}

/// Result type of handlers: an error is itself a response.
pub type ApiResult = Result<ApiResponse, ApiResponse>;

/// Default and maximum `per_page`, as GitHub applies them.
pub const PER_PAGE_DEFAULT: usize = 30;
// Source: <https://docs.github.com/en/rest/using-the-rest-api/using-pagination-in-the-rest-api#changing-the-number-of-items-per-page>
pub const PER_PAGE_MAX: usize = 100;

/// Returns one page of `items` with GitHub's `Link` header.
pub fn paginate<T: serde::Serialize>(
    request: &ApiRequest,
    path: &str,
    items: Vec<T>,
) -> ApiResponse {
    paginate_with(request, path, items, |_, page| page)
}

/// Paginates a collection while wrapping each page in a typed response envelope.
pub fn paginate_with<T: serde::Serialize, R: serde::Serialize>(
    request: &ApiRequest,
    path: &str,
    items: Vec<T>,
    wrap: impl FnOnce(i64, Vec<T>) -> R,
) -> ApiResponse {
    let total_count = i64::try_from(items.len()).unwrap_or(i64::MAX);
    let (body, link) = page(request, path, items);
    let response = ApiResponse::ok_typed(&wrap(total_count, body));
    match link {
        Some(link) => response.with_header("link", link),
        None => response,
    }
}

/// The requested page of `items` and the `Link` header pointing at its neighbours.
///
/// Links keep every query parameter of the request except `page` and `per_page`, as GitHub's do.
pub fn page<T>(request: &ApiRequest, path: &str, items: Vec<T>) -> (Vec<T>, Option<String>) {
    let per_page = request
        .query("per_page")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(PER_PAGE_DEFAULT)
        .clamp(1, PER_PAGE_MAX);
    let page = request
        .query("page")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(1)
        .max(1);
    let last = items.len().div_ceil(per_page).max(1);
    let body: Vec<T> = items
        .into_iter()
        .skip((page - 1).saturating_mul(per_page))
        .take(per_page)
        .collect();
    let kept = request
        .query
        .iter()
        .filter(|(name, _)| name != "page" && name != "per_page")
        .collect::<Vec<_>>();
    let link = |target: usize, rel: &str| {
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        for (name, value) in &kept {
            query.append_pair(name, value);
        }
        query.append_pair("per_page", &per_page.to_string());
        query.append_pair("page", &target.to_string());
        let url = request.urls().api(&format!("{path}?{}", query.finish()));
        format!("<{url}>; rel=\"{rel}\"")
    };
    let mut links = Vec::new();
    // Source: <https://docs.github.com/en/rest/using-the-rest-api/using-pagination-in-the-rest-api#using-link-headers>
    if page < last {
        links.push(link(page + 1, "next"));
        links.push(link(last, "last"));
    }
    if page > 1 {
        links.push(link(1, "first"));
        links.push(link(page - 1, "prev"));
    }
    (body, (!links.is_empty()).then(|| links.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(query: &str) -> ApiRequest {
        ApiRequest {
            path: "/user/repos".to_owned(),
            params: HashMap::new(),
            query: url::form_urlencoded::parse(query.as_bytes())
                .into_owned()
                .collect(),
            headers: http::HeaderMap::new(),
            body: bytes::Bytes::new(),
            caller: None,
            client: None,
            api_url: "http://h/api/v3".to_owned(),
            web_url: "http://h".to_owned(),
        }
    }

    fn page(query: &str, count: usize) -> (Vec<Value>, Option<String>) {
        let items = (0..count).map(|index| json!(index)).collect();
        let response = paginate(&request(query), "/user/repos", items);
        let Body::Json(Value::Array(body)) = response.body else {
            panic!("not an array");
        };
        let link = response
            .headers
            .into_iter()
            .find(|(name, _)| name == "link")
            .map(|(_, value)| value);
        (body, link)
    }

    #[test]
    fn first_page_links_next_and_last() {
        let (body, link) = page("per_page=2", 5);
        assert_eq!(body, vec![json!(0), json!(1)]);
        let link = link.unwrap();
        assert!(link.contains("<http://h/api/v3/user/repos?per_page=2&page=2>; rel=\"next\""));
        assert!(link.contains("page=3>; rel=\"last\""));
        assert!(!link.contains("prev"));
    }

    #[test]
    fn last_page_links_back_only() {
        let (body, link) = page("per_page=2&page=3", 5);
        assert_eq!(body, vec![json!(4)]);
        let link = link.unwrap();
        assert!(link.contains("rel=\"prev\"") && link.contains("rel=\"first\""));
        assert!(!link.contains("next"));
    }

    #[test]
    fn links_keep_the_other_query_parameters() {
        let (_, link) = page("state=all&sort=updated&per_page=2", 5);
        assert_eq!(
            link.unwrap(),
            "<http://h/api/v3/user/repos?state=all&sort=updated&per_page=2&page=2>; rel=\"next\", \
             <http://h/api/v3/user/repos?state=all&sort=updated&per_page=2&page=3>; rel=\"last\""
        );
    }

    #[test]
    fn single_page_has_no_link_and_per_page_is_capped() {
        let (body, link) = page("", 5);
        assert_eq!(body.len(), 5);
        assert!(link.is_none());
        let (body, _) = page("per_page=1000", 150);
        assert_eq!(body.len(), PER_PAGE_MAX);
    }
}
