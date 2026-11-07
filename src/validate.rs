//! Checks requests and responses against the pinned spec, and webhook payloads against the
//! pinned octokit/webhooks schemas, so the fake cannot silently drift from GitHub.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

use jsonschema::error::{TypeKind, ValidationErrorKind};
use jsonschema::{JsonType, Registry, ValidationError, Validator};
use mime::Mime;
use serde_json::{Value, json};
use winnow::Parser as _;
use winnow::combinator::{alt, repeat};
use winnow::error::ModalResult;
use winnow::token::{literal, take_till, take_while};

use crate::api::{ApiResponse, DOCS_URL};
use crate::operation::OperationInfo;
use crate::types;

/// The pinned spec with `spec/overrides.json` applied and OpenAPI's `nullable` rewritten as
/// JSON Schema. `build.rs` writes the patched spec, which is embedded in the binary.
fn spec() -> &'static Value {
    static SPEC: OnceLock<Value> = OnceLock::new();
    SPEC.get_or_init(|| {
        let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/api.github.com.json"));
        let mut spec: Value =
            serde_json::from_slice(bytes).expect("the pinned spec is not valid JSON");
        rewrite_nullable(&mut spec);
        spec
    })
}

/// Turns every `{"nullable": true, ...}` into `{"anyOf": [{...}, {"type": "null"}]}`.
fn rewrite_nullable(value: &mut Value) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(rewrite_nullable),
        Value::Object(object) => {
            object.values_mut().for_each(rewrite_nullable);
            if object.remove("nullable") == Some(Value::Bool(true)) {
                let inner = Value::Object(std::mem::take(object));
                object.insert("anyOf".to_owned(), json!([inner, { "type": "null" }]));
            }
        }
        _ => {}
    }
}

/// Follows a local `$ref` such as `#/components/responses/not_found`.
fn resolve(value: &Value) -> &Value {
    match value.get("$ref").and_then(Value::as_str) {
        Some(reference) => {
            let pointer = reference.strip_prefix('#').unwrap_or(reference);
            spec().pointer(pointer).unwrap_or(&Value::Null)
        }
        None => value,
    }
}

/// The operation's object in the spec, such as `paths./repos/{owner}/{repo}.patch`.
fn operation(info: &OperationInfo) -> &'static Value {
    let method = info.method.as_str().to_ascii_lowercase();
    &spec()["paths"][info.path][method.as_str()]
}

/// The JSON schema of `status` for an operation, if the spec documents a JSON body for it.
fn response_schema(info: &OperationInfo, status: u16) -> Result<Option<&'static Value>, String> {
    let responses = &operation(info)["responses"];
    let response = responses
        .get(status.to_string())
        .map(resolve)
        .ok_or_else(|| format!("{} does not document status {status}", info.id))?;
    Ok(response["content"]
        .as_object()
        .and_then(|content| content.iter().find(|(media, _)| is_json(media)))
        .map(|(_, media)| &media["schema"]))
}

/// Whether a media type is JSON, as `application/json` and `application/vnd.github+json` are.
fn is_json(media: &str) -> bool {
    let Ok(parsed) = media.parse::<Mime>() else {
        return false;
    };
    parsed.subtype() == mime::JSON || parsed.suffix() == Some(mime::JSON)
}

/// The octokit/webhooks payload schemas as one draft-07 document, each file a definition
/// such as `push-event` or `commit`. `build.rs` writes it, and it is embedded in the binary.
fn webhook_schemas() -> &'static Value {
    static SCHEMAS: OnceLock<Value> = OnceLock::new();
    SCHEMAS.get_or_init(|| {
        let bytes = include_bytes!(concat!(env!("OUT_DIR"), "/webhooks.json"));
        serde_json::from_slice(bytes).expect("the webhook schemas are not valid JSON")
    })
}

/// Where the registry keeps the patched spec, against which spec schemas resolve `$ref`s.
const SPEC_URI: &str = "urn:ninot-github:spec";

/// Where the registry keeps the webhook schemas.
const WEBHOOKS_URI: &str = "urn:ninot-github:webhooks";

/// The spec and the webhook schemas, indexed once and borrowed by every validator so that none
/// holds its own copy. Each is read under the draft its `$schema` names, 2020-12 without one.
fn registry() -> &'static Registry<'static> {
    static REGISTRY: OnceLock<Registry<'static>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        Registry::new()
            .add(SPEC_URI, spec())
            .and_then(|builder| builder.add(WEBHOOKS_URI, webhook_schemas()))
            .and_then(|builder| builder.prepare())
            .expect("the embedded schemas index")
    })
}

/// Compiled validators by what they check, such as `repos/get 200` or `push event`.
type Cache = HashMap<String, Arc<Validator>>;

/// The validator cached under `key`, compiled from `document` on first use.
fn cached(key: String, document: impl FnOnce() -> Value) -> Result<Arc<Validator>, String> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    let cache = CACHE.get_or_init(Default::default);
    if let Some(validator) = cache.lock().unwrap().get(&key) {
        return Ok(validator.clone());
    }
    let validator = jsonschema::options()
        .with_registry(registry())
        .build(&document())
        .map_err(|error| format!("cannot compile the schema of {key}: {error}"))?;
    let validator = Arc::new(validator);
    cache.lock().unwrap().insert(key, validator.clone());
    Ok(validator)
}

/// Points every document-local `$ref` in `schema` at the same location in the resource `base`.
fn anchor_refs(schema: &mut Value, base: &str) {
    match schema {
        Value::Array(items) => items.iter_mut().for_each(|item| anchor_refs(item, base)),
        Value::Object(object) => {
            if let Some(Value::String(reference)) = object.get_mut("$ref")
                && reference.starts_with('#')
            {
                *reference = format!("{base}{reference}");
            }
            object
                .values_mut()
                .for_each(|value| anchor_refs(value, base));
        }
        _ => {}
    }
}

/// The validator of the spec schema `schema`, cached under `key`.
fn validator(key: String, schema: &Value) -> Result<Arc<Validator>, String> {
    cached(key, || {
        let mut response = schema.clone();
        anchor_refs(&mut response, SPEC_URI);
        json!({
            "$schema": "https://json-schema.org/draft/2020-12/schema",
            "$ref": "#/$defs/response",
            "$defs": { "response": response },
        })
    })
}

/// Up to ten ways `body` violates `validator`, or `Ok` when it conforms.
fn errors(validator: &Validator, what: &str, body: &Value) -> Result<(), String> {
    let errors: Vec<String> = validator
        .iter_errors(body)
        .take(10)
        .map(|error| format!("{} at {}", error, error.instance_path()))
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "{what} does not match the spec:\n  {}",
            errors.join("\n  ")
        ))
    }
}

/// Checks a successful JSON response of `info` against the spec.
pub(crate) fn check(info: &'static OperationInfo, status: u16, body: &Value) -> Result<(), String> {
    if !(200..300).contains(&status) {
        return Ok(());
    }
    let Some(schema) = response_schema(info, status)? else {
        return Err(format!(
            "{} documents no JSON body for status {status}",
            info.id
        ));
    };
    let what = format!("{} {status}", info.id);
    errors(&*validator(what.clone(), schema)?, &what, body)
}

/// Checks a request body against the operation's `application/json` request schema, as GitHub
/// does before running the operation. An empty body counts as `{}`, and an operation without
/// such a schema, such as `repos/get-content` whose `GET` documents only examples, accepts any.
///
/// # Errors
///
/// A body that is not JSON is GitHub's `400` "Problems parsing JSON". One that violates the
/// schema is a `422` listing every violation in the wording of GitHub's validator, such as
/// `Invalid request.\n\nFor 'properties/draft', "yes" is not a boolean.`
pub(crate) fn check_request(info: &'static OperationInfo, body: &[u8]) -> Result<(), ApiResponse> {
    let operation = operation(info);
    let Some(schema) = resolve(&operation["requestBody"])["content"]
        .get("application/json")
        .and_then(|media| media.get("schema"))
    else {
        return Ok(());
    };
    let documentation_url = operation["externalDocs"]["url"]
        .as_str()
        .unwrap_or(DOCS_URL);
    let refusal = |status: u16, message: String| {
        ApiResponse::typed(
            status,
            &types::BasicError {
                documentation_url: Some(documentation_url.to_owned()),
                message: Some(message),
                status: Some(status.to_string()),
                url: None,
            },
        )
    };
    let body: Value = if body.is_empty() {
        json!({})
    } else {
        serde_json::from_slice(body)
            .map_err(|_| refusal(400, "Problems parsing JSON".to_owned()))?
    };
    let validator = validator(format!("{} request", info.id), schema)
        .map_err(|error| ApiResponse::error(500, &error))?;
    let mut lines = Vec::new();
    let mut missing = BTreeMap::new();
    for error in validator.iter_errors(&body) {
        describe(&error, &mut lines, &mut missing);
    }
    for mut properties in missing.into_values() {
        properties.sort();
        let verb = if properties.len() == 1 {
            "wasn't"
        } else {
            "weren't"
        };
        lines.push(format!(
            "\"{}\" {verb} supplied.",
            properties.join("\", \"")
        ));
    }
    if lines.is_empty() {
        Ok(())
    } else {
        Err(refusal(
            422,
            format!("Invalid request.\n\n{}", lines.join("\n")),
        ))
    }
}

/// Adds GitHub's wording of `error` to `lines`, or the missing property it names to `missing`
/// by the instance path of its object.
fn describe(
    error: &ValidationError<'_>,
    lines: &mut Vec<String>,
    missing: &mut BTreeMap<String, Vec<String>>,
) {
    let at = error.instance_path().to_string();
    let instance = error.instance();
    lines.push(match error.kind() {
        ValidationErrorKind::Required { property } => {
            let property = property.as_str().unwrap_or_default().to_owned();
            missing.entry(at).or_default().push(property);
            return;
        }
        ValidationErrorKind::Type { kind } => {
            let kinds: Vec<&str> = match kind {
                TypeKind::Single(kind) => vec![kind.as_str()],
                TypeKind::Multiple(kinds) => kinds.iter().map(|kind| kind.as_str()).collect(),
            };
            let kinds: Vec<String> = kinds
                .into_iter()
                .map(|kind| match kind {
                    "array" | "integer" | "object" => format!("an {kind}"),
                    _ => format!("a {kind}"),
                })
                .collect();
            format!(
                "For '{}', {} is not {}.",
                schema_key(&at),
                inspect(instance),
                kinds.join(" or ")
            )
        }
        ValidationErrorKind::Enum { options } => {
            let shown = match &**instance {
                Value::String(text) => text.clone(),
                Value::Null => String::new(),
                other => inspect(other),
            };
            format!("{shown} is not a member of {}.", inspect(options))
        }
        ValidationErrorKind::AdditionalProperties { unexpected } => {
            let mut keys = unexpected.clone();
            keys.sort();
            let (verb, noun) = if keys.len() == 1 {
                ("is not a", "key")
            } else {
                ("are not", "keys")
            };
            format!("\"{}\" {verb} permitted {noun}.", keys.join("\", \""))
        }
        ValidationErrorKind::AnyOf { context } => {
            if let [first, second] = &context[..]
                && let Some(errors) = not_null(first, second)
            {
                for error in errors {
                    describe(error, lines, missing);
                }
                return;
            }
            "No subschema in \"anyOf\" matched.".to_owned()
        }
        ValidationErrorKind::OneOfNotValid { .. } => {
            "No subschema in \"oneOf\" matched.".to_owned()
        }
        ValidationErrorKind::OneOfMultipleValid { .. } => {
            "More than one subschema in \"oneOf\" matched.".to_owned()
        }
        _ => format!("For '{}', {error}", schema_key(&at)),
    });
}

/// The errors of the branch that is not `{"type": "null"}`, when one of two `anyOf` branches is,
/// as [`rewrite_nullable`] makes them. A non-null value then violates only the other branch.
fn not_null<'a>(
    first: &'a [ValidationError<'static>],
    second: &'a [ValidationError<'static>],
) -> Option<&'a [ValidationError<'static>]> {
    let is_null_branch = |errors: &[ValidationError<'_>]| {
        matches!(
            errors,
            [error] if matches!(
                error.kind(),
                ValidationErrorKind::Type { kind: TypeKind::Single(JsonType::Null) }
            )
        )
    };
    if is_null_branch(first) {
        Some(second)
    } else if is_null_branch(second) {
        Some(first)
    } else {
        None
    }
}

/// Where GitHub's validator says a value sits, such as `properties/config/properties/url` for
/// the instance path `/config/url`, and `links/0/schema` for the body itself.
fn schema_key(instance_path: &str) -> String {
    if instance_path.is_empty() {
        return "links/0/schema".to_owned();
    }
    let segments: Vec<String> = pointer_segments.parse(instance_path).unwrap_or_default();
    segments
        .iter()
        .map(|segment| {
            if index.parse(segment.as_str()).is_ok() {
                "items".to_owned()
            } else {
                format!("properties/{segment}")
            }
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// The segments of a JSON Pointer, such as `/config/url`, with its `~0` and `~1` escapes
/// resolved: `a~1b` reads as `a/b`.
fn pointer_segments(input: &mut &str) -> ModalResult<Vec<String>> {
    repeat(0.., ('/', pointer_segment).map(|(_, segment)| segment)).parse_next(input)
}

/// One JSON Pointer segment. A `~` that escapes nothing is kept as it is.
fn pointer_segment(input: &mut &str) -> ModalResult<String> {
    repeat(
        0..,
        alt((
            literal("~1").value("/"),
            literal("~0").value("~"),
            literal("~").value("~"),
            take_till(1.., ('~', '/')),
        )),
    )
    .fold(String::new, |mut segment: String, piece| {
        segment.push_str(piece);
        segment
    })
    .parse_next(input)
}

/// A segment that is an array index rather than a property name.
fn index<'s>(input: &mut &'s str) -> ModalResult<&'s str> {
    take_while(0.., '0'..='9').parse_next(input)
}

/// A JSON value as Ruby's `inspect` shows it, which GitHub's messages quote, such as `nil` or
/// `{"a" => [1, "b"]}`.
fn inspect(value: &Value) -> String {
    match value {
        Value::Null => "nil".to_owned(),
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(inspect).collect();
            format!("[{}]", items.join(", "))
        }
        Value::Object(object) => {
            let pairs: Vec<String> = object
                .iter()
                .map(|(key, value)| format!("{} => {}", Value::from(key.as_str()), inspect(value)))
                .collect();
            format!("{{{}}}", pairs.join(", "))
        }
        other => other.to_string(),
    }
}

/// Checks `body` against the component schema `name`, such as an error body against
/// `basic-error`.
#[cfg(test)]
pub(crate) fn check_component(name: &str, body: &Value) -> Result<(), String> {
    if spec()["components"]["schemas"].get(name).is_none() {
        return Err(format!("the spec has no schema {name}"));
    }
    let schema = json!({ "$ref": format!("#/components/schemas/{name}") });
    errors(&*validator(name.to_owned(), &schema)?, name, body)
}

/// One definition from the embedded octokit/webhooks bundle, such as `push-event` or
/// `pull_request-opened`.
pub(crate) fn webhook_definition(name: &str) -> Option<&'static Value> {
    webhook_schemas()["definitions"].get(name)
}

/// `"{event}-event"` when that aggregate schema exists, otherwise `"{event}-{action}"` taken
/// from `body`. Ping, push, create, delete and status use the aggregate form; the other events
/// are published as one schema per action.
fn webhook_schema_name(event: &str, body: &Value) -> Option<String> {
    let definitions = &webhook_schemas()["definitions"];
    let event_name = format!("{event}-event");
    if definitions.get(&event_name).is_some() {
        return Some(event_name);
    }
    let action = body.get("action").and_then(Value::as_str)?;
    let action_name = format!("{event}-{action}");
    definitions
        .get(&action_name)
        .is_some()
        .then_some(action_name)
}

/// Checks the payload `body` of `event`, such as `push`, against its octokit/webhooks schema.
pub(crate) fn check_webhook(event: &str, body: &Value) -> Result<(), String> {
    let Some(name) = webhook_schema_name(event, body) else {
        return Err(format!("no webhook schema for {event}"));
    };
    let what = format!("{event} event");
    let validator = cached(format!("webhook {name}"), || {
        json!({
            "$schema": "http://json-schema.org/draft-07/schema#",
            "$ref": format!("{WEBHOOKS_URI}#/definitions/{name}"),
        })
    })?;
    errors(&validator, &what, body)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::operation::{Operation, ops};

    #[test]
    fn nullable_becomes_any_of_null() {
        let mut schema = json!({ "type": "string", "nullable": true });
        rewrite_nullable(&mut schema);
        assert_eq!(
            schema,
            json!({ "anyOf": [{ "type": "string" }, { "type": "null" }] })
        );
    }

    #[test]
    fn broken_responses_fail_and_undocumented_statuses_are_reported() {
        let info = <ops::RateLimitGet as Operation>::INFO;
        let error = check(info, 200, &json!({ "rate": "nope" })).unwrap_err();
        assert!(
            error.contains("rate-limit/get 200 does not match the spec"),
            "{error}"
        );
        assert!(
            check(info, 201, &json!({}))
                .unwrap_err()
                .contains("does not document status 201")
        );
        assert!(check(info, 404, &json!({})).is_ok());
    }

    /// Resident memory of this process in bytes.
    #[cfg(target_os = "linux")]
    fn resident_bytes() -> usize {
        let statm = std::fs::read_to_string("/proc/self/statm").unwrap();
        let pages: usize = statm.split_whitespace().nth(1).unwrap().parse().unwrap();
        pages * 4096
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn validators_share_the_spec_instead_of_copying_it() {
        let documented = crate::handlers::IMPLEMENTED
            .iter()
            .flat_map(|info| {
                (200..300).filter_map(move |status| {
                    response_schema(info, status)
                        .ok()
                        .flatten()
                        .map(|schema| (format!("{} {status}", info.id), schema))
                })
            })
            .collect::<Vec<_>>();
        assert!(documented.len() > 50, "{}", documented.len());
        let (first, rest) = documented.split_first().unwrap();
        validator(first.0.clone(), first.1).unwrap();
        let before = resident_bytes();
        for (key, schema) in rest {
            validator(key.clone(), schema).unwrap();
        }
        let grown = resident_bytes().saturating_sub(before);
        assert!(
            grown < 64 << 20,
            "{} validators grew memory by {} MiB",
            rest.len(),
            grown >> 20
        );
    }

    #[test]
    fn large_referenced_schemas_are_enforced() {
        let info = <ops::ReposGet as Operation>::INFO;
        let error = check(info, 200, &json!({ "id": 1 })).unwrap_err();
        assert!(
            error.contains("\"archive_url\" is a required property"),
            "{error}"
        );
    }

    #[test]
    fn component_schemas_are_enforced() {
        assert!(check_component("basic-error", &json!({ "message": "Not Found" })).is_ok());
        let error = check_component("hook", &json!({ "id": "1" })).unwrap_err();
        assert!(error.contains("hook does not match the spec"), "{error}");
        assert!(
            check_component("no-such-schema", &json!({}))
                .unwrap_err()
                .contains("the spec has no schema no-such-schema")
        );
    }

    #[test]
    fn webhook_schemas_are_enforced() {
        let error = check_webhook("ping", &json!({ "hook_id": "1" })).unwrap_err();
        assert!(error.contains("ping event does not match"), "{error}");
        assert!(error.contains("\"zen\" is a required property"), "{error}");
        assert!(
            check_webhook("no-such-event", &json!({}))
                .unwrap_err()
                .contains("no webhook schema for no-such-event")
        );
    }

    #[test]
    fn push_pushers_require_an_email() {
        let validator = cached("committer".to_owned(), || {
            let mut document = webhook_schemas().clone();
            document["$ref"] = "#/definitions/committer".into();
            document
        })
        .unwrap();
        assert!(
            errors(
                &validator,
                "pusher",
                &json!({ "name": "admin", "email": null })
            )
            .is_ok()
        );
        let error = errors(&validator, "pusher", &json!({ "name": "admin" })).unwrap_err();
        assert!(
            error.contains("\"email\" is a required property"),
            "{error}"
        );
    }

    #[test]
    fn error_bodies_match_their_schemas() {
        use crate::api::{ApiResponse, Body};
        let json = |response: ApiResponse| match response.body {
            Body::Json(body) => body,
            _ => panic!("not a JSON body"),
        };
        for response in [ApiResponse::not_found(), ApiResponse::error(501, "x")] {
            check_component("basic-error", &json(response)).unwrap();
        }
        let failed = ApiResponse::validation_failed("Repository", "name", "custom");
        check_component("validation-error", &json(failed)).unwrap();
        let info = <ops::ReposCreateForAuthenticatedUser as Operation>::INFO;
        let unparseable = check_request(info, b"not json").unwrap_err();
        check_component("basic-error", &json(unparseable)).unwrap();
        let invalid = check_request(info, br#"{"name": "tool", "private": "yes"}"#).unwrap_err();
        check_component("validation-error", &json(invalid)).unwrap();
    }

    #[test]
    fn values_are_shown_as_ruby_inspects_them() {
        assert_eq!(
            inspect(&json!({ "a": [1, "b", null, true, 1.5] })),
            r#"{"a" => [1, "b", nil, true, 1.5]}"#
        );
    }

    #[test]
    fn nested_values_are_located_by_schema_path() {
        assert_eq!(schema_key(""), "links/0/schema");
        assert_eq!(
            schema_key("/config/url"),
            "properties/config/properties/url"
        );
        assert_eq!(schema_key("/events/0"), "properties/events/items");
    }

    #[test]
    fn request_violations_are_listed_in_one_message() {
        let info = <ops::ReposUpdateWebhook as Operation>::INFO;
        assert!(check_request(info, b"").is_ok());
        let refused = check_request(
            info,
            br#"{"config": {"url": 5}, "events": ["push", 1], "active": null}"#,
        )
        .unwrap_err();
        let crate::api::Body::Json(body) = refused.body else {
            panic!("a 422 has a JSON body");
        };
        let message = body["message"].as_str().unwrap();
        for line in [
            "For 'properties/config/properties/url', 5 is not a string.",
            "For 'properties/events/items', 1 is not a string.",
            "For 'properties/active', nil is not a boolean.",
        ] {
            assert!(message.contains(line), "{message}");
        }
        let info = <ops::ReposGet as Operation>::INFO;
        assert!(check_request(info, b"not json").is_ok());
    }

    #[test]
    fn overrides_are_enforced() {
        let error = check_component(
            "validation-error",
            &json!({ "message": "Validation Failed", "documentation_url": "x" }),
        )
        .unwrap_err();
        assert!(
            error.contains("\"status\" is a required property"),
            "{error}"
        );
    }
}
