//! Checks that every API URL a response carries names a path of the pinned OpenAPI description.
//!
//! The fake answers at its root as well as under `/api/v3`, so a test that calls the root gets
//! API and web URLs on the same base. A URL under `/api/v3` or `/api/uploads` is an API URL; one
//! directly under the base is taken for an API URL when its first segment starts a spec path
//! of several segments, such as `repos` or `users`, or when it is a one-segment spec path, such
//! as `/octocat`. Web URLs, such as `{base}/octocat/hello/pull/1`, are not checked.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use serde_json::Value;

/// URLs GitHub emits whose path the spec lacks, with the reason each is kept.
const NOT_IN_SPEC: &[(&str, &str)] = &[
    (
        r"^/repos/[^/]+/[^/]+/hooks/[^/]+/test$",
        "hook test_url: OpenAPI schema hook's example is .../hooks/1/test, while \
         repos/test-push-webhook is at .../hooks/{hook_id}/tests",
    ),
    (
        r"^/repos/[^/]+/[^/]+/downloads$",
        "repository downloads_url: OpenAPI schema repository's example; the downloads API it \
         named is no longer in the spec",
    ),
    (
        r"^/repos/[^/]+/[^/]+/x$",
        "repository archive_url .../{archive_format}{/ref}: OpenAPI schema repository's example; \
         it expands to repos/download-tarball-archive or repos/download-zipball-archive",
    ),
];

struct Spec {
    /// Anchored patterns of the spec's path templates.
    paths: Vec<regress::Regex>,
    /// First segments of the spec's paths of several segments, such as `repos`.
    roots: BTreeSet<String>,
    /// The spec's one-segment paths, such as `octocat`.
    leaves: BTreeSet<String>,
    allowed: Vec<regress::Regex>,
}

fn spec() -> &'static Spec {
    static SPEC: OnceLock<Spec> = OnceLock::new();
    SPEC.get_or_init(|| {
        let file = concat!(env!("CARGO_MANIFEST_DIR"), "/spec/api.github.com.json");
        let spec: Value = serde_json::from_slice(&std::fs::read(file).unwrap()).unwrap();
        let multi = multi_segment(&spec);
        let mut paths = Vec::new();
        let mut roots = BTreeSet::new();
        let mut leaves = BTreeSet::new();
        for path in spec["paths"].as_object().unwrap().keys() {
            match path.trim_start_matches('/').split_once('/') {
                Some((root, _)) => roots.insert(root.to_owned()),
                None => leaves.insert(path.trim_start_matches('/').to_owned()),
            };
            paths.push(regress::Regex::new(&pattern(path, &multi)).unwrap());
        }
        let allowed = NOT_IN_SPEC
            .iter()
            .map(|(pattern, _)| regress::Regex::new(pattern).unwrap())
            .collect();
        Spec {
            paths,
            roots,
            leaves,
            allowed,
        }
    })
}

/// `(path, parameter)` pairs the spec marks `x-multi-segment`.
fn multi_segment(spec: &Value) -> BTreeSet<(String, String)> {
    let mut found = BTreeSet::new();
    for (path, item) in spec["paths"].as_object().unwrap() {
        for operation in item.as_object().unwrap().values() {
            for parameter in operation["parameters"].as_array().into_iter().flatten() {
                let parameter = match parameter["$ref"].as_str() {
                    Some(reference) => {
                        &spec["components"]["parameters"]
                            [reference.trim_start_matches("#/components/parameters/")]
                    }
                    None => parameter,
                };
                if parameter["in"] == "path"
                    && (parameter["x-multi-segment"] == true
                        || parameter["schema"]["x-multi-segment"] == true)
                {
                    found.insert((path.clone(), parameter["name"].as_str().unwrap().to_owned()));
                }
            }
        }
    }
    found
}

/// An anchored pattern for `template`: a parameter matches one segment, or several when
/// multi-segment. A `sha` parameter matches one: the spec marks some multi-segment because
/// they accept a ref, but a URL the fake emits holds an object id there.
fn pattern(template: &str, multi: &BTreeSet<(String, String)>) -> String {
    let mut out = String::from("^");
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let end = start + rest[start..].find('}').unwrap();
        out.push_str(&escape(&rest[..start]));
        let name = &rest[start + 1..end];
        if multi.contains(&(template.to_owned(), name.to_owned())) && !name.ends_with("sha") {
            out.push_str(".+");
        } else {
            out.push_str("[^/]+");
        }
        rest = &rest[end + 1..];
    }
    out.push_str(&escape(rest));
    out.push('$');
    out
}

fn escape(text: &str) -> String {
    text.chars()
        .flat_map(|c| {
            let special = "\\^$.|?*+()[]{}/".contains(c);
            special.then_some('\\').into_iter().chain([c])
        })
        .collect()
}

/// `url` as a path to match: URI-template expressions that add segments or a query
/// (`{/number}`, `{?since}`, `{&page}`) dropped, the others (`{sha}`, `{+path}`) replaced by
/// `x`, then the query and fragment cut.
fn normalize(path: &str) -> String {
    let mut out = String::new();
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        let Some(len) = rest[start..].find('}') else {
            break;
        };
        out.push_str(&rest[..start]);
        let expression = &rest[start + 1..start + len];
        if !expression.starts_with(['/', '?', '&', '#', '.']) {
            out.push('x');
        }
        rest = &rest[start + len + 1..];
    }
    out.push_str(rest);
    let end = out.find(['?', '#']).unwrap_or(out.len());
    out.truncate(end);
    out
}

/// The path of `url` relative to `base` when `url` is an API URL, as the module explains.
fn api_path<'u>(base: &str, url: &'u str) -> Option<&'u str> {
    let rest = url.strip_prefix(base)?;
    for prefix in ["/api/v3", "/api/uploads"] {
        if let Some(path) = rest
            .strip_prefix(prefix)
            .filter(|path| path.starts_with('/'))
        {
            return Some(path);
        }
    }
    let path = normalize(rest);
    let path = path.strip_prefix('/')?;
    let api = match path.split_once('/') {
        Some((root, _)) => spec().roots.contains(root),
        None => spec().leaves.contains(path),
    };
    api.then_some(rest)
}

/// The origin of a URL on a loopback server, such as `http://127.0.0.1:4000`.
fn loopback_origin(url: &str) -> Option<&str> {
    let rest = url.strip_prefix("http://127.0.0.1:")?;
    let port = rest
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(rest.len());
    Some(&url[.."http://127.0.0.1:".len() + port])
}

/// Every API URL under `base` in `body` that names no spec path, as `pointer: url`. With no
/// `base`, every loopback server is a base, as in a webhook payload.
pub fn unknown_api_urls(base: Option<&str>, body: &Value) -> Vec<String> {
    let mut found = Vec::new();
    walk(body, String::new(), &mut |pointer, url| {
        let Some(base) = base.or_else(|| loopback_origin(url)) else {
            return;
        };
        let Some(path) = api_path(base, url) else {
            return;
        };
        let path = normalize(path);
        let spec = spec();
        let known = spec.paths.iter().any(|p| p.find(&path).is_some())
            || spec.allowed.iter().any(|p| p.find(&path).is_some());
        if !known {
            found.push(format!("{pointer}: {url}"));
        }
    });
    found
}

fn walk(value: &Value, pointer: String, visit: &mut dyn FnMut(&str, &str)) {
    match value {
        Value::String(text) => visit(&pointer, text),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                walk(item, format!("{pointer}/{index}"), visit);
            }
        }
        Value::Object(fields) => {
            for (key, item) in fields {
                walk(item, format!("{pointer}/{key}"), visit);
            }
        }
        _ => {}
    }
}

/// Panics when `body`, the reply to `request`, carries an API URL no spec path names.
pub fn assert_spec_urls(base: Option<&str>, request: &str, body: &Value) {
    let unknown = unknown_api_urls(base, body);
    assert!(
        unknown.is_empty(),
        "{request} returned API URLs that match no path of spec/api.github.com.json:\n{}",
        unknown.join("\n")
    );
}

#[test]
fn spec_url_guard_accepts_spec_paths_and_rejects_others() {
    let base = "http://h:1";
    let body = serde_json::json!({
        "url": "http://h:1/repos/o/r/pulls/1",
        "html_url": "http://h:1/o/r/pull/1",
        "issues_url": "http://h:1/repos/o/r/issues{/number}",
        "compare_url": "http://h:1/repos/o/r/compare/{base}...{head}",
        "contents_url": "http://h:1/api/v3/repos/o/r/contents/{+path}",
        "branch": "http://h:1/repos/o/r/branches/a/b",
        "upload_url": "http://h:1/api/uploads/repos/o/r/releases/1/assets{?name,label}",
        "notifications_url": "http://h:1/repos/o/r/notifications{?since,all,participating}",
        "other": "https://example.com/repos/o/r/nope",
        "bad": ["http://h:1/repos/o/r/nope/1"],
        "status": "http://h:1/repos/o/r/statuses/abc/1",
    });
    assert_eq!(
        unknown_api_urls(Some(base), &body),
        [
            "/bad/0: http://h:1/repos/o/r/nope/1",
            "/status: http://h:1/repos/o/r/statuses/abc/1"
        ]
    );
    let payload = serde_json::json!({
        "url": "http://127.0.0.1:5/repos/o/r/nope",
        "hook": "http://127.0.0.1:6/hook",
        "repository": { "url": "http://127.0.0.1:5/repos/o/r" },
    });
    assert_eq!(
        unknown_api_urls(None, &payload),
        ["/url: http://127.0.0.1:5/repos/o/r/nope"]
    );
}
