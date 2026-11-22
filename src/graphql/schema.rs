//! Dynamic schema construction and GraphQL request execution.

#![expect(
    clippy::disallowed_types,
    clippy::disallowed_methods,
    reason = "untyped JSON; remove when this module is typed"
)]

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex, OnceLock};

use async_graphql::dynamic::{
    Enum, EnumItem, Field, FieldFuture, FieldValue, InputObject, InputValue, Interface,
    InterfaceField, Object, ResolverContext, Scalar, Schema, TypeRef, Union,
};
use async_graphql::extensions::{
    Extension, ExtensionContext, ExtensionFactory, NextResolve, ResolveInfo,
};
use async_graphql::parser::parse_schema;
use async_graphql::parser::types::{
    BaseType, ConstDirective, FieldDefinition, InputValueDefinition, Type, TypeKind,
    TypeSystemDefinition,
};
use async_graphql::{Error, PathSegment, Positioned, QueryPathSegment, ServerResult};
use serde_json::Value as Json;
use winnow::Parser;

use super::{Env, Map, Resolver, SDL, grammar};
use crate::api::{ApiResponse, Caller};
use crate::server::Hit;
use crate::state::State;

/// How a field's named type turns JSON into a field value.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shape {
    /// A scalar or enum: the JSON is the value.
    Leaf,
    /// An object: the JSON object is the parent of its fields.
    Object,
    /// An interface or union: like [`Shape::Object`], typed by its `__typename`.
    Abstract,
}

/// Converts a resolved JSON value into a field value of the given shape.
fn field_value(value: Json, shape: Shape) -> Option<FieldValue<'static>> {
    match (value, shape) {
        (Json::Null, _) => None,
        (Json::Array(items), _) => {
            Some(FieldValue::list(items.into_iter().map(|item| {
                field_value(item, shape).unwrap_or(FieldValue::NULL)
            })))
        }
        (Json::Object(object), Shape::Object) => Some(FieldValue::owned_any(object)),
        (Json::Object(object), Shape::Abstract) => {
            let type_name = object
                .get("__typename")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_owned();
            Some(FieldValue::owned_any(object).with_type(type_name))
        }
        (leaf, _) => Some(FieldValue::value(
            async_graphql::Value::from_json(leaf).expect("JSON converts to a GraphQL value"),
        )),
    }
}

/// Resolves the field `key` (`Type.field`) of the parent in `ctx`.
fn resolve(
    ctx: &ResolverContext<'_>,
    key: &str,
    shape: Shape,
    resolver: Option<Resolver>,
) -> Result<Option<FieldValue<'static>>, Error> {
    let empty = Map::new();
    let parent = ctx.parent_value.downcast_ref::<Map>().unwrap_or(&empty);
    let (_, name) = grammar::field_key.parse(key).unwrap_or(("", key));
    let value = match resolver {
        Some(resolver) => {
            let args: Map = ctx
                .args
                .iter()
                .map(|(argument, value)| {
                    let value = value.as_value().clone().into_json();
                    let value = value.expect("arguments parsed from JSON convert back to JSON");
                    (argument.to_string(), value)
                })
                .collect();
            resolver(ctx.data_unchecked::<Env>(), parent, &args)?.0
        }
        None => match parent.get(name).cloned() {
            Some(value) => value,
            // Connections store `nodes`. `edges` is the same page with cursors.
            None if name == "edges" => super::edges::from_nodes(parent)
                .ok_or_else(|| super::errors::not_implemented(key))?,
            None => {
                return Err(super::errors::not_implemented(key));
            }
        },
    };
    Ok(field_value(value, shape))
}

/// The dynamic type of `ty`.
fn type_ref(ty: &Type) -> TypeRef {
    let base = match &ty.base {
        BaseType::Named(name) => TypeRef::Named(name.to_string().into()),
        BaseType::List(item) => TypeRef::List(Box::new(type_ref(item))),
    };
    if ty.nullable {
        base
    } else {
        TypeRef::NonNull(Box::new(base))
    }
}

/// The named type inside any list and non-null wrappers of `ty`.
fn base_name(ty: &Type) -> &str {
    match &ty.base {
        BaseType::Named(name) => name.as_str(),
        BaseType::List(item) => base_name(item),
    }
}

/// The reason of a `@deprecated` directive: `None` when not deprecated.
fn deprecation(directives: &[Positioned<ConstDirective>]) -> Option<Option<String>> {
    directives
        .iter()
        .find(|directive| directive.node.name.node == "deprecated")
        .map(|directive| {
            directive
                .node
                .get_argument("reason")
                .and_then(|reason| match &reason.node {
                    async_graphql::Value::String(reason) => Some(reason.clone()),
                    _ => None,
                })
        })
}

/// Applies an optional description and deprecation to a dynamic item.
macro_rules! annotate {
    ($item:expr, $description:expr) => {
        match $description {
            Some(description) => $item.description(description.node),
            None => $item,
        }
    };
    ($item:expr, $description:expr, $directives:expr) => {{
        let item = annotate!($item, $description);
        match deprecation($directives) {
            Some(reason) => item.deprecation(reason.as_deref()),
            None => item,
        }
    }};
}

/// An argument or input field.
fn input_value(definition: InputValueDefinition) -> InputValue {
    let mut value = InputValue::new(definition.name.node.as_str(), type_ref(&definition.ty.node));
    if let Some(default) = definition.default_value {
        value = value.default_value(default.node);
    }
    annotate!(value, definition.description, &definition.directives)
}

/// An object field, resolved by `resolver` or read from the parent object.
fn object_field(
    type_name: &str,
    definition: FieldDefinition,
    shapes: &HashMap<String, Shape>,
    resolver: Option<Resolver>,
) -> Field {
    let key: Arc<str> = format!("{type_name}.{}", definition.name.node).into();
    let shape = shapes
        .get(base_name(&definition.ty.node))
        .copied()
        .unwrap_or(Shape::Leaf);
    let nullable = definition.ty.node.nullable;
    let mut field = Field::new(
        definition.name.node.as_str(),
        type_ref(&definition.ty.node),
        move |ctx| {
            let result = match resolve(&ctx, &key, shape, resolver) {
                // async-graphql drops a failed field from its object; GitHub makes it null.
                Err(error) if nullable => {
                    ctx.add_error(ctx.set_error_path(error.into_server_error(ctx.item.pos)));
                    Ok(None)
                }
                result => result,
            };
            FieldFuture::new(async move { result })
        },
    );
    for argument in definition.arguments {
        field = field.argument(input_value(argument.node));
    }
    annotate!(field, definition.description, &definition.directives)
}

/// An interface field.
fn interface_field(definition: FieldDefinition) -> InterfaceField {
    let mut field =
        InterfaceField::new(definition.name.node.as_str(), type_ref(&definition.ty.node));
    for argument in definition.arguments {
        field = field.argument(input_value(argument.node));
    }
    annotate!(field, definition.description, &definition.directives)
}

/// Builds the schema from [`SDL`], wiring [`RESOLVERS`] into their fields.
///
/// # Errors
///
/// Fails if the SDL does not parse or async-graphql rejects the resulting schema, or if a
/// resolver names a field the SDL lacks.
pub(super) fn build_schema() -> Result<Schema, String> {
    let document = parse_schema(SDL).map_err(|error| error.to_string())?;
    let types: Vec<_> = document
        .definitions
        .into_iter()
        .filter_map(|definition| match definition {
            TypeSystemDefinition::Type(definition) => Some(definition.node),
            _ => None,
        })
        .collect();
    let shapes: HashMap<String, Shape> = types
        .iter()
        .filter_map(|definition| {
            let shape = match definition.kind {
                TypeKind::Object(_) => Shape::Object,
                TypeKind::Interface(_) | TypeKind::Union(_) => Shape::Abstract,
                _ => return None,
            };
            Some((definition.name.node.to_string(), shape))
        })
        .collect();
    let interface_fields: HashMap<(String, String), Type> = types
        .iter()
        .filter_map(|definition| match &definition.kind {
            TypeKind::Interface(interface) => Some((&definition.name.node, &interface.fields)),
            _ => None,
        })
        .flat_map(|(name, fields)| {
            fields.iter().map(|field| {
                let key = (name.to_string(), field.node.name.node.to_string());
                (key, field.node.ty.node.clone())
            })
        })
        .collect();
    let mut resolvers: HashMap<&str, Resolver> = super::resolvers().into_iter().collect();
    let mut builder = Schema::build("Query", Some("Mutation"), None);
    for definition in types {
        let name = definition.name.node.as_str();
        builder = match definition.kind {
            TypeKind::Scalar => {
                builder.register(annotate!(Scalar::new(name), definition.description))
            }
            TypeKind::Object(object) => {
                let mut dynamic = Object::new(name);
                for interface in &object.implements {
                    dynamic = dynamic.implement(interface.node.as_str());
                }
                for mut field in object.fields {
                    // async-graphql rejects a field narrower than its interface's, such as
                    // `User.email: String!` for `ProfileOwner.email: String`.
                    if let Some(ty) = object.implements.iter().find_map(|interface| {
                        interface_fields
                            .get(&(interface.node.to_string(), field.node.name.node.to_string()))
                    }) {
                        field.node.ty.node = ty.clone();
                    }
                    let resolver =
                        resolvers.remove(format!("{name}.{}", field.node.name.node).as_str());
                    dynamic = dynamic.field(object_field(name, field.node, &shapes, resolver));
                }
                builder.register(annotate!(dynamic, definition.description))
            }
            TypeKind::Interface(interface) => {
                let mut dynamic = Interface::new(name);
                for implemented in interface.implements {
                    dynamic = dynamic.implement(implemented.node.as_str());
                }
                for field in interface.fields {
                    dynamic = dynamic.field(interface_field(field.node));
                }
                builder.register(annotate!(dynamic, definition.description))
            }
            TypeKind::Union(union) => {
                let mut dynamic = Union::new(name);
                for member in union.members {
                    dynamic = dynamic.possible_type(member.node.as_str());
                }
                builder.register(annotate!(dynamic, definition.description))
            }
            TypeKind::Enum(enumeration) => {
                let mut dynamic = Enum::new(name);
                for value in enumeration.values {
                    let value = value.node;
                    dynamic = dynamic.item(annotate!(
                        EnumItem::new(value.value.node.as_str()),
                        value.description,
                        &value.directives
                    ));
                }
                builder.register(annotate!(dynamic, definition.description))
            }
            TypeKind::InputObject(input) => {
                let mut dynamic = InputObject::new(name);
                for field in input.fields {
                    dynamic = dynamic.field(input_value(field.node));
                }
                builder.register(annotate!(dynamic, definition.description))
            }
        };
    }
    if let Some(key) = resolvers.keys().next() {
        return Err(format!("resolver {key} names a field the schema lacks"));
    }
    builder
        .extension(ErrorPaths)
        .extension(RecordFields)
        .finish()
        .map_err(|error| error.to_string())
}

/// Sets the `path` of errors from non-null fields, which async-graphql's dynamic schema leaves
/// empty.
struct ErrorPaths;

impl ExtensionFactory for ErrorPaths {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(ErrorPaths)
    }
}

#[async_graphql::async_trait::async_trait]
impl Extension for ErrorPaths {
    async fn resolve(
        &self,
        ctx: &ExtensionContext<'_>,
        info: ResolveInfo<'_>,
        next: NextResolve<'_>,
    ) -> ServerResult<Option<async_graphql::Value>> {
        let node = info.path_node;
        next.run(ctx, info).await.map_err(|mut error| {
            if error.path.is_empty() {
                let mut path: Vec<PathSegment> = std::iter::once(node)
                    .chain(node.parents())
                    .map(|node| match node.segment {
                        QueryPathSegment::Index(index) => PathSegment::Index(index),
                        QueryPathSegment::Name(name) => PathSegment::Field(name.to_owned()),
                    })
                    .collect();
                path.reverse();
                error.path = path;
            }
            error
        })
    }
}

/// The schema, built on first use and shared by every server in the process.
fn schema() -> &'static Schema {
    static SCHEMA: OnceLock<Schema> = OnceLock::new();
    SCHEMA.get_or_init(|| build_schema().expect("GitHub's GraphQL schema builds"))
}

/// Where [`RecordFields`] puts the fields a request resolved.
struct Hits(Arc<Mutex<BTreeSet<Hit>>>);

/// Records each field of an object or interface that resolves without error as a
/// [`Hit::GraphQl`], leaving out introspection and the items of lists.
struct RecordFields;

impl ExtensionFactory for RecordFields {
    fn create(&self) -> Arc<dyn Extension> {
        Arc::new(RecordFields)
    }
}

#[async_graphql::async_trait::async_trait]
impl Extension for RecordFields {
    async fn resolve(
        &self,
        ctx: &ExtensionContext<'_>,
        info: ResolveInfo<'_>,
        next: NextResolve<'_>,
    ) -> ServerResult<Option<async_graphql::Value>> {
        let recorded = !info.is_for_introspection
            && !info.name.starts_with("__")
            && !info.parent_type.starts_with('[');
        let field = recorded.then(|| format!("{}.{}", info.parent_type, info.name));
        let result = next.run(ctx, info).await;
        if let (Some(field), Ok(_), Some(Hits(hits))) = (field, &result, ctx.data_opt::<Hits>()) {
            hits.lock().unwrap().insert(Hit::GraphQl { field });
        }
        result
    }
}

/// Every field of the schema's object and interface types, as `Type.field`.
pub fn fields() -> Vec<String> {
    let query = r#"{"query": "{ __schema { types { name kind fields { name } } } }"}"#;
    let request: async_graphql::Request = serde_json::from_str(query).unwrap();
    let response = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("cannot start a runtime")
        .block_on(schema().execute(request));
    let body = serde_json::to_value(&response.data).unwrap();
    let mut fields = Vec::new();
    for kind in body["__schema"]["types"].as_array().into_iter().flatten() {
        let name = kind["name"].as_str().unwrap_or_default();
        if name.starts_with("__") || !matches!(kind["kind"].as_str(), Some("OBJECT" | "INTERFACE"))
        {
            continue;
        }
        for field in kind["fields"].as_array().into_iter().flatten() {
            fields.push(format!(
                "{name}.{}",
                field["name"].as_str().unwrap_or_default()
            ));
        }
    }
    fields.sort();
    fields
}

/// The `Type.field` of an `Unknown field "field" on type "Type".` validation error.
fn unknown_field(message: &str) -> Option<String> {
    let (type_name, field) = grammar::unknown_field(&mut &*message).ok()?;
    Some(format!("{type_name}.{field}"))
}

/// Executes one GraphQL request body and returns GitHub's response shape: errors carry their
/// `type` at the top level rather than under `extensions`.
pub fn execute(
    state: &Arc<Mutex<State>>,
    hits: &Arc<Mutex<BTreeSet<Hit>>>,
    caller: Option<Caller>,
    web_url: &str,
    body: &[u8],
) -> ApiResponse {
    let Some(caller) = caller else {
        return crate::errors::GithubError::GraphqlUnauthenticated.to_response();
    };
    let request: async_graphql::Request = match serde_json::from_slice(body) {
        Ok(request) => request,
        Err(error) => {
            return crate::errors::GithubError::ProblemsParsingJsonDetail {
                detail: error.to_string(),
            }
            .to_response();
        }
    };
    let request = request
        .data(Env {
            state: state.clone(),
            caller,
            web_url: web_url.to_owned(),
        })
        .data(Hits(hits.clone()));
    let response = tokio::runtime::Handle::current().block_on(schema().execute(request));
    // Errors without a path come from parsing or validation; resolvers' errors are answers.
    for error in response.errors.iter().filter(|error| error.path.is_empty()) {
        hits.lock().unwrap().insert(Hit::GraphQlError {
            field: unknown_field(&error.message),
            message: error.message.clone(),
        });
    }
    let mut body = serde_json::to_value(&response).unwrap_or(Json::Null);
    if let Some(errors) = body.get_mut("errors").and_then(Json::as_array_mut) {
        for error in errors {
            if let Some(kind) = error.pointer("/extensions/type").cloned() {
                error["type"] = kind;
            }
        }
    }
    ApiResponse::ok(body)
}

#[cfg(test)]
mod tests {
    #[test]
    fn github_schema_builds() {
        super::build_schema().unwrap();
    }
}
