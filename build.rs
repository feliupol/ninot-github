//! Generates typed REST request and response models from the pinned GitHub OpenAPI description,
//! and webhook payload types from the pinned octokit/webhooks schemas. The operation table comes
//! from [`ninot_common::spec`].

use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use proc_macro2::TokenStream;
use quote::ToTokens;
use serde::Deserialize;
use serde_json::{Value, json};

const SPEC: &str = "spec/api.github.com.json";

/// Corrections to component schemas where the spec is looser than GitHub.
const OVERRIDES: &str = "spec/overrides.json";

/// Component schemas the handlers return. Their Rust types, and those of every schema they
/// reference, are generated into `types`.
const RESPONSE_SCHEMAS: [&str; 27] = [
    "actions-public-key",
    "actions-secret",
    "basic-error",
    "base-gist",
    "branch-with-protection",
    "commit",
    "commit-comparison",
    "content-directory",
    "content-file",
    "content-submodule",
    "content-symlink",
    "full-repository",
    "git-ref",
    "hook",
    "hook-delivery-item",
    "issue-search-result-item",
    "minimal-repository",
    "organization-full",
    "public-user",
    "rate-limit-overview",
    "release",
    "release-asset",
    "repository",
    "root",
    "short-branch",
    "simple-user",
    "validation-error",
];

/// Supported operations whose JSON request models are generated alongside their responses.
const REQUEST_OPERATIONS: [&str; 24] = [
    "actions/create-or-update-repo-secret",
    "actions/re-run-workflow",
    "actions/re-run-workflow-failed-jobs",
    "apps/create-installation-access-token",
    "gists/create",
    "git/create-ref",
    "issues/add-labels",
    "issues/create",
    "issues/create-comment",
    "pulls/create",
    "pulls/create-review",
    "pulls/merge",
    "pulls/remove-requested-reviewers",
    "pulls/request-reviewers",
    "pulls/update",
    "repos/create-for-authenticated-user",
    "repos/create-in-org",
    "repos/create-or-update-file-contents",
    "repos/create-release",
    "repos/create-webhook",
    "repos/transfer",
    "repos/update",
    "repos/update-release",
    "repos/update-webhook",
];

/// Supported REST operations whose JSON response models are used by handlers.
const RESPONSE_OPERATIONS: [&str; 51] = [
    "actions/create-or-update-repo-secret",
    "actions/get-repo-public-key",
    "actions/get-repo-secret",
    "actions/list-repo-secrets",
    "actions/get-job-for-workflow-run",
    "actions/get-workflow-run",
    "actions/list-jobs-for-workflow-run",
    "actions/list-workflow-runs-for-repo",
    "actions/re-run-workflow",
    "actions/re-run-workflow-failed-jobs",
    "apps/create-installation-access-token",
    "apps/get-repo-installation",
    "checks/get",
    "checks/list-for-ref",
    "gists/create",
    "git/create-ref",
    "issues/add-labels",
    "issues/create",
    "issues/create-comment",
    "issues/get",
    "issues/list-for-repo",
    "issues/list-labels-for-repo",
    "issues/list-comments",
    "issues/list-events-for-timeline",
    "pulls/create",
    "pulls/create-review",
    "pulls/get",
    "pulls/list",
    "pulls/list-commits",
    "pulls/list-files",
    "pulls/list-reviews",
    "pulls/list-review-comments",
    "pulls/list-requested-reviewers",
    "pulls/merge",
    "pulls/remove-requested-reviewers",
    "pulls/request-reviewers",
    "pulls/update",
    "repos/create-for-authenticated-user",
    "repos/create-in-org",
    "repos/create-or-update-file-contents",
    "repos/create-release",
    "repos/create-webhook",
    "repos/transfer",
    "repos/update",
    "repos/update-release",
    "repos/update-webhook",
    "repos/get-combined-status-for-ref",
    "repos/list-commit-statuses-for-ref",
    "repos/list-pull-requests-associated-with-commit",
    "search/issues-and-pull-requests",
    "search/repos",
];

/// Payload schemas from octokit/webhooks, `payload-schemas/api.github.com` of the commit in
/// `SOURCE`, trimmed to the events below and the schemas they reference.
const WEBHOOK_SCHEMAS: &str = "spec/webhooks";

/// Events the fake delivers. Each has a schema `<event>/event.schema.json`.
const WEBHOOK_EVENTS: [&str; 2] = ["ping", "push"];

/// One correction to the component schema `schema` at the JSON pointer `path` inside it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Override {
    schema: String,
    path: String,
    /// Properties to add, which must not exist yet.
    #[serde(default)]
    properties: serde_json::Map<String, Value>,
    /// Properties to mark required, which must exist.
    #[serde(default)]
    required: Vec<String>,
    /// The GitHub behaviour the override copies, and its source.
    #[allow(dead_code, reason = "documentation for readers of the overrides file")]
    reason: String,
}

fn main() {
    println!("cargo::rerun-if-changed={SPEC}");
    println!("cargo::rerun-if-changed={OVERRIDES}");
    println!("cargo::rerun-if-changed={WEBHOOK_SCHEMAS}");
    let mut spec: Value =
        serde_json::from_slice(&fs::read(SPEC).expect("cannot read the pinned spec"))
            .expect("the pinned spec is not valid JSON");
    let overrides: Vec<Override> =
        serde_json::from_slice(&fs::read(OVERRIDES).expect("cannot read the spec overrides"))
            .expect("the spec overrides are malformed");
    apply_overrides(&mut spec, overrides);
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    // `validate` checks responses against this patched copy.
    fs::write(
        out.join("api.github.com.json"),
        serde_json::to_vec(&spec).unwrap(),
    )
    .expect("cannot write the patched spec");
    fs::write(
        out.join("operations.rs"),
        ninot_common::spec::operations(&spec),
    )
    .expect("cannot write the generated operations");

    let types = api_types(&spec);
    let file = syn::parse2(types).expect("the generated types are not valid Rust");
    fs::write(out.join("types.rs"), prettyplease::unparse(&file))
        .expect("cannot write the generated types");

    let definitions = webhook_definitions(Path::new(WEBHOOK_SCHEMAS));
    // `validate` checks payloads against this bundle.
    let bundle = json!({
        "$schema": "http://json-schema.org/draft-07/schema",
        "definitions": definitions,
    });
    fs::write(
        out.join("webhooks.json"),
        serde_json::to_vec(&bundle).unwrap(),
    )
    .expect("cannot write the webhook schemas");
    let file = syn::parse2(webhook_types(definitions))
        .expect("the generated webhook types are not valid Rust");
    fs::write(out.join("webhook_types.rs"), prettyplease::unparse(&file))
        .expect("cannot write the generated webhook types");
}

/// The schemas under `dir` as definitions of one document, keyed by [`definition_name`], with
/// their references rewritten to `#/definitions/<name>`.
fn webhook_definitions(dir: &Path) -> BTreeMap<String, Value> {
    let mut schemas = BTreeMap::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(path) = pending.pop() {
        if path.is_dir() {
            for entry in fs::read_dir(&path).expect("cannot list the webhook schemas") {
                pending.push(entry.expect("cannot list the webhook schemas").path());
            }
        } else if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            let mut schema: Value =
                serde_json::from_slice(&fs::read(&path).expect("cannot read a webhook schema"))
                    .unwrap_or_else(|error| panic!("{} is not JSON: {error}", path.display()));
            let object = schema
                .as_object_mut()
                .unwrap_or_else(|| panic!("{} is not a schema object", path.display()));
            object.remove("$schema");
            let id = object
                .remove("$id")
                .and_then(|id| id.as_str().map(str::to_owned))
                .unwrap_or_else(|| panic!("{} has no $id", path.display()));
            // References resolve against the `$id`, relative to the schemas directory.
            let base = id.rsplit_once('/').map_or("", |(base, _)| base);
            rewrite_references(&mut schema, base);
            schemas.insert(definition_name(&id), schema);
        }
    }
    for event in WEBHOOK_EVENTS {
        assert!(
            schemas.contains_key(&format!("{event}-event")),
            "{WEBHOOK_SCHEMAS} has no schema for {event}"
        );
    }
    schemas
}

/// Names the schema `push$event` `push-event` and `common/commit.schema.json` `commit`.
fn definition_name(id: &str) -> String {
    let file = id.rsplit('/').next().unwrap_or(id);
    file.strip_suffix(".schema.json")
        .unwrap_or(file)
        .replace('$', "-")
}

/// Rewrites every reference inside `value` to another schema file, relative to `base`, as a
/// reference to its definition.
fn rewrite_references(value: &mut Value, base: &str) {
    match value {
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| rewrite_references(item, base)),
        Value::Object(object) => {
            if let Some(Value::String(reference)) = object.get_mut("$ref") {
                assert!(
                    !reference.contains('#'),
                    "reference {reference} has a fragment"
                );
                let mut segments: Vec<&str> = base.split('/').filter(|s| !s.is_empty()).collect();
                for segment in reference.split('/') {
                    match segment {
                        ".." => {
                            segments.pop();
                        }
                        "." => {}
                        segment => segments.push(segment),
                    }
                }
                *reference = format!("#/definitions/{}", definition_name(&segments.join("/")));
            }
            object
                .values_mut()
                .for_each(|item| rewrite_references(item, base));
        }
        _ => {}
    }
}

/// Generates serde types for the webhook payload `definitions`.
fn webhook_types(mut definitions: BTreeMap<String, Value>) -> TokenStream {
    // Without `additionalProperties: false` typify generates types that ignore unknown fields,
    // so converting a response type to a payload type drops what the payload does not carry.
    // `validate` still rejects payloads with extra fields.
    definitions
        .values_mut()
        .for_each(allow_additional_properties);
    let definitions = definitions.into_iter().map(|(name, schema)| {
        let schema: schemars::schema::Schema = serde_json::from_value(schema)
            .unwrap_or_else(|error| panic!("webhook schema {name} is not JSON Schema: {error}"));
        (name, schema)
    });
    let mut space = typify::TypeSpace::new(&typify::TypeSpaceSettings::default());
    space
        .add_ref_types(definitions)
        .expect("typify cannot convert the webhook schemas");
    space.to_stream()
}

/// Removes `additionalProperties: false` from `value` and every schema inside it.
fn allow_additional_properties(value: &mut Value) {
    match value {
        Value::Array(items) => items.iter_mut().for_each(allow_additional_properties),
        Value::Object(object) => {
            if object.get("additionalProperties") == Some(&Value::Bool(false)) {
                object.remove("additionalProperties");
            }
            object.values_mut().for_each(allow_additional_properties);
        }
        _ => {}
    }
}

/// Applies `overrides` to the component schemas of `spec`.
fn apply_overrides(spec: &mut Value, overrides: Vec<Override>) {
    for patch in overrides {
        let pointer = format!("/components/schemas/{}{}", patch.schema, patch.path);
        let object = spec
            .pointer_mut(&pointer)
            .and_then(Value::as_object_mut)
            .unwrap_or_else(|| panic!("override target {pointer} is not a schema object"));
        let properties = object
            .entry("properties")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .unwrap_or_else(|| panic!("{pointer}/properties is not an object"));
        for (name, schema) in patch.properties {
            assert!(
                properties.insert(name.clone(), schema).is_none(),
                "override adds {name} to {pointer}, which already has it"
            );
        }
        for name in &patch.required {
            assert!(
                properties.contains_key(name),
                "override requires {name}, which {pointer} does not define"
            );
        }
        let required = object
            .entry("required")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap_or_else(|| panic!("{pointer}/required is not an array"));
        for name in patch.required {
            if !required.contains(&Value::String(name.clone())) {
                required.push(name.into());
            }
        }
    }
}

/// Generates typed models for supported responses and JSON request bodies.
fn api_types(spec: &Value) -> TokenStream {
    let mut schemas: BTreeMap<String, Value> = spec["components"]["schemas"]
        .as_object()
        .expect("the spec has no component schemas")
        .iter()
        .map(|(name, schema)| (name.clone(), schema.clone()))
        .collect();
    let paths = spec["paths"].as_object().expect("the spec has no paths");
    let mut roots: Vec<String> = RESPONSE_SCHEMAS
        .iter()
        .map(|name| (*name).to_owned())
        .collect();
    let mut request_fields = BTreeMap::new();
    let mut nullable_bodies = Vec::new();
    for operation_id in REQUEST_OPERATIONS {
        let (method, operation) = paths
            .values()
            .filter_map(Value::as_object)
            .flat_map(|methods| methods.iter())
            .find(|(_, operation)| operation["operationId"] == operation_id)
            .unwrap_or_else(|| panic!("the spec has no operation {operation_id}"));
        let schema = operation["requestBody"]["content"]["application/json"]["schema"]
            .as_object()
            .unwrap_or_else(|| panic!("operation {operation_id} has no JSON body schema"));
        let name = request_type_name(operation_id);
        let body_nullable = schema.get("nullable") == Some(&Value::Bool(true));
        let schema_name = if body_nullable {
            format!("{name}Body")
        } else {
            name.clone()
        };
        let mut schema = Value::Object(schema.clone());
        schema.as_object_mut().unwrap().remove("nullable");
        if operation_id == "pulls/request-reviewers"
            && let Some(object) = schema.as_object_mut()
        {
            // The schema's anyOf permits either property but not both, while GitHub accepts both.
            // Keep both optional in the model and enforce at least one in the handler.
            object.remove("anyOf");
        }
        if operation_id == "repos/update-webhook" {
            let config_name = "ReposUpdateWebhookRequestConfig";
            let mut config = spec["components"]["schemas"]["webhook-config"].clone();
            config["properties"]["secret"]["nullable"] = Value::Bool(true);
            schemas.insert(config_name.to_owned(), config.clone());
            roots.push(config_name.to_owned());
            request_fields.insert(
                config_name.to_owned(),
                RequestFields::from_schema(&config, false),
            );
            schema["properties"]["config"] = json!({
                "$ref": format!("#/components/schemas/{config_name}")
            });
        }
        let mut request_rules =
            RequestFields::from_schema(&schema, method.eq_ignore_ascii_case("patch"));
        if operation_id == "repos/update" {
            request_rules
                .nullable
                .extend(["description".to_owned(), "homepage".to_owned()]);
        }
        request_fields.insert(schema_name.clone(), request_rules);
        schemas.insert(schema_name.clone(), schema);
        roots.push(schema_name.clone());
        if body_nullable {
            nullable_bodies.push((name, schema_name));
        }
    }

    for operation_id in RESPONSE_OPERATIONS {
        let operation = paths
            .values()
            .filter_map(Value::as_object)
            .flat_map(|methods| methods.values())
            .find(|operation| operation["operationId"] == operation_id)
            .unwrap_or_else(|| panic!("the spec has no operation {operation_id}"));
        let responses = operation["responses"]
            .as_object()
            .expect("operation responses are an object");
        let schema = responses
            .iter()
            .filter(|(status, _)| status.starts_with('2'))
            .find_map(|(_, response)| response["content"]["application/json"]["schema"].as_object())
            .unwrap_or_else(|| panic!("operation {operation_id} has no JSON success schema"));
        let name = response_type_name(operation_id);
        schemas.insert(name.clone(), Value::Object(schema.clone()));
        roots.push(name.clone());
    }

    let mut generated = generate_types(schemas, roots, request_fields);
    for (name, inner) in nullable_bodies {
        generated.extend(
            TokenStream::from_str(&format!("pub type {name} = Option<{inner}>;"))
                .expect("nullable body alias is valid Rust"),
        );
    }
    generated
}

/// The field presence rules for one request object schema.
#[derive(Default)]
struct RequestFields {
    /// Properties that the pinned schema requires.
    required: BTreeSet<String>,
    /// Optional properties that accept an explicit `null` separately from omission.
    nullable: BTreeSet<String>,
    /// Whether omission means no update rather than applying schema defaults.
    patch: bool,
}

impl RequestFields {
    /// Reads required and nullable properties from an operation's body schema.
    fn from_schema(schema: &Value, patch: bool) -> Self {
        let required = schema["required"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect();
        let nullable = schema["properties"]
            .as_object()
            .into_iter()
            .flat_map(|properties| properties.iter())
            .filter(|(_, property)| property["nullable"] == true)
            .map(|(name, _)| name.clone())
            .collect();
        Self {
            required,
            nullable,
            patch,
        }
    }
}

/// Names the generated request type for an OpenAPI operation ID.
fn request_type_name(operation_id: &str) -> String {
    let name = operation_id
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut characters = part.chars();
            characters
                .next()
                .map(|first| first.to_ascii_uppercase().to_string() + characters.as_str())
                .unwrap_or_default()
        })
        .collect::<String>();
    format!("{name}Request")
}

/// Names the generated response type for an OpenAPI operation ID.
fn response_type_name(operation_id: &str) -> String {
    let request_name = request_type_name(operation_id);
    format!("{}Response", request_name.trim_end_matches("Request"))
}

/// Generates the schemas reachable from `roots` with typify.
fn generate_types(
    mut schemas: BTreeMap<String, Value>,
    roots: Vec<String>,
    request_fields: BTreeMap<String, RequestFields>,
) -> TokenStream {
    // OpenAPI 3.0's `nullable` is not JSON Schema. A nullable component becomes non-null and
    // every reference to it nullable, so the named type and its nullable uses stay distinct.
    let nullable: BTreeSet<String> = schemas
        .iter_mut()
        .filter(|(_, schema)| {
            schema.as_object().and_then(|object| object.get("nullable")) == Some(&Value::Bool(true))
        })
        .map(|(name, schema)| {
            schema.as_object_mut().unwrap().remove("nullable");
            name.clone()
        })
        .collect();
    for schema in schemas.values_mut() {
        rewrite_nullable(schema, &nullable);
    }

    let mut needed = BTreeSet::new();
    let mut pending = roots;
    while let Some(name) = pending.pop() {
        let schema = schemas
            .get(&name)
            .unwrap_or_else(|| panic!("the spec has no schema {name}"));
        if needed.insert(name) {
            referenced(schema, &mut pending);
        }
    }
    let definitions = needed.into_iter().map(|name| {
        let schema: schemars::schema::Schema = serde_json::from_value(schemas[&name].clone())
            .unwrap_or_else(|error| panic!("API schema {name} is invalid: {error}"));
        (name, schema)
    });
    let mut space = typify::TypeSpace::new(&typify::TypeSpaceSettings::default());
    space
        .add_ref_types(definitions)
        .unwrap_or_else(|error| panic!("typify cannot convert the API schemas: {error}"));
    let mut file =
        syn::parse2::<syn::File>(space.to_stream()).expect("typify generated invalid API models");
    let mut changed_fields = BTreeMap::<String, BTreeSet<String>>::new();
    for item in &mut file.items {
        let syn::Item::Struct(item) = item else {
            continue;
        };
        let Some(fields) = request_fields.get(&item.ident.to_string()) else {
            continue;
        };
        let syn::Fields::Named(named) = &mut item.fields else {
            continue;
        };
        for field in &mut named.named {
            let Some(ident) = &field.ident else {
                continue;
            };
            let name = wire_field_name(field).unwrap_or_else(|| ident.to_string());
            if fields.required.contains(&name) {
                continue;
            }
            let optional = option_inner(&field.ty);
            let nullable = fields.nullable.contains(&name);
            if optional.is_none() && !fields.patch && !nullable {
                continue;
            }
            let inner = optional.unwrap_or_else(|| field.ty.clone());
            if fields.patch {
                remove_serde_default(&mut field.attrs);
                changed_fields
                    .entry(item.ident.to_string())
                    .or_default()
                    .insert(name.clone());
            }
            if nullable {
                remove_serde_skip_serializing_if(&mut field.attrs);
                field.ty = syn::parse_quote!(Option<Option<#inner>>);
                field.attrs.push(syn::parse_quote!(
                    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::types::deserialize_nullable_optional")]
                ));
            } else if fields.patch {
                remove_serde_skip_serializing_if(&mut field.attrs);
                field.ty = syn::parse_quote!(Option<#inner>);
                field.attrs.push(syn::parse_quote!(
                    #[serde(default, skip_serializing_if = "Option::is_none", deserialize_with = "crate::types::deserialize_optional")]
                ));
            } else {
                field.attrs.push(syn::parse_quote!(
                    #[serde(default, deserialize_with = "crate::types::deserialize_optional")]
                ));
            }
        }
    }
    wrap_patch_default_values(&mut file.items, changed_fields);
    TokenStream::from_str(&prettyplease::unparse(&file))
        .expect("prettyplease output was not valid Rust tokens")
}

/// Reads a generated field's serialized name, such as `type` for the Rust identifier `type_`.
fn wire_field_name(field: &syn::Field) -> Option<String> {
    field.attrs.iter().find_map(|attribute| {
        if !attribute.path().is_ident("serde") {
            return None;
        }
        let syn::Meta::List(list) = &attribute.meta else {
            return None;
        };
        let metas = list
            .parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
            )
            .ok()?;
        metas.into_iter().find_map(|meta| {
            let syn::Meta::NameValue(value) = meta else {
                return None;
            };
            if !value.path.is_ident("rename") {
                return None;
            }
            let syn::Expr::Lit(expression) = value.value else {
                return None;
            };
            let syn::Lit::Str(name) = expression.lit else {
                return None;
            };
            Some(name.value())
        })
    })
}

/// Keeps generated `Default` implementations valid after PATCH fields become optional.
fn wrap_patch_default_values(
    items: &mut [syn::Item],
    changed_fields: BTreeMap<String, BTreeSet<String>>,
) {
    for item in items {
        let syn::Item::Impl(implementation) = item else {
            continue;
        };
        let Some((_, trait_path, _)) = &implementation.trait_ else {
            continue;
        };
        if trait_path
            .segments
            .last()
            .is_none_or(|segment| segment.ident != "Default")
        {
            continue;
        }
        let syn::Type::Path(self_type) = implementation.self_ty.as_ref() else {
            continue;
        };
        let Some(fields) = self_type
            .path
            .segments
            .last()
            .and_then(|segment| changed_fields.get(&segment.ident.to_string()))
        else {
            continue;
        };
        for member in implementation.items.iter_mut() {
            let syn::ImplItem::Fn(method) = member else {
                continue;
            };
            if method.sig.ident != "default" {
                continue;
            }
            for statement in &mut method.block.stmts {
                let syn::Stmt::Expr(syn::Expr::Struct(structure), _) = statement else {
                    continue;
                };
                for field in &mut structure.fields {
                    let syn::Member::Named(name) = &field.member else {
                        continue;
                    };
                    if fields.contains(&name.to_string()) {
                        let value = field.expr.clone();
                        let is_default = matches!(
                            &value,
                            syn::Expr::Call(call)
                                if matches!(call.func.as_ref(), syn::Expr::Path(path)
                                    if path.path.segments.last().is_some_and(|segment| segment.ident == "default"))
                        );
                        if !is_default {
                            field.expr = syn::parse_quote!(Some(#value));
                        }
                    }
                }
            }
        }
    }
}

/// Removes typify's `Vec::is_empty` serializer when a PATCH vector becomes optional.
fn remove_serde_skip_serializing_if(attributes: &mut [syn::Attribute]) {
    for attribute in attributes.iter_mut() {
        if !attribute.path().is_ident("serde") {
            continue;
        }
        let syn::Meta::List(list) = &mut attribute.meta else {
            continue;
        };
        let metas = list
            .parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
            )
            .expect("generated serde attributes parse");
        let metas = metas
            .into_iter()
            .filter(|meta| !meta.path().is_ident("skip_serializing_if"))
            .collect::<syn::punctuated::Punctuated<_, syn::Token![,]>>();
        list.tokens = metas.into_token_stream();
    }
}

/// Removes a schema default from a PATCH field, where omission means “leave unchanged”.
fn remove_serde_default(attributes: &mut [syn::Attribute]) {
    for attribute in attributes.iter_mut() {
        if !attribute.path().is_ident("serde") {
            continue;
        }
        let syn::Meta::List(list) = &mut attribute.meta else {
            continue;
        };
        let mut metas = list
            .parse_args_with(
                syn::punctuated::Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated,
            )
            .expect("generated serde attributes parse");
        metas = metas
            .into_iter()
            .filter(|meta| !meta.path().is_ident("default"))
            .collect();
        list.tokens = metas.into_token_stream();
    }
}

/// Returns the inner type of an `Option<T>` field.
fn option_inner(ty: &syn::Type) -> Option<syn::Type> {
    let syn::Type::Path(path) = ty else {
        return None;
    };
    let segment = path.path.segments.last()?;
    if segment.ident != "Option" {
        return None;
    }
    let syn::PathArguments::AngleBracketed(arguments) = &segment.arguments else {
        return None;
    };
    match arguments.args.first()? {
        syn::GenericArgument::Type(inner) => Some(inner.clone()),
        _ => None,
    }
}

/// Rewrites `nullable: true` as `anyOf` with `null`, and references to `nullable` components
/// likewise.
fn rewrite_nullable(value: &mut Value, nullable: &BTreeSet<String>) {
    match value {
        Value::Array(items) => items
            .iter_mut()
            .for_each(|item| rewrite_nullable(item, nullable)),
        Value::Object(object) => {
            let reference = object.get("$ref").and_then(Value::as_str);
            let nullable_reference = reference.is_some_and(|reference| {
                nullable.contains(reference.rsplit('/').next().unwrap_or(reference))
            });
            if !nullable_reference {
                object
                    .values_mut()
                    .for_each(|item| rewrite_nullable(item, nullable));
            }
            if nullable_reference || object.remove("nullable") == Some(Value::Bool(true)) {
                let inner = Value::Object(std::mem::take(object));
                object.insert("anyOf".to_owned(), json!([inner, { "type": "null" }]));
            }
        }
        _ => {}
    }
}

/// Collects the names of the components `value` references.
fn referenced(value: &Value, names: &mut Vec<String>) {
    match value {
        Value::Array(items) => items.iter().for_each(|item| referenced(item, names)),
        Value::Object(object) => {
            if let Some(reference) = object.get("$ref").and_then(Value::as_str) {
                names.push(reference.rsplit('/').next().unwrap_or(reference).to_owned());
            }
            object.values().for_each(|item| referenced(item, names));
        }
        _ => {}
    }
}
