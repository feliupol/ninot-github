//! Projects a JSON value onto an octokit/webhooks definition.
//!
//! REST models carry fields the webhook schema rejects, and omit nulls the schema requires.
//! Walking the schema drops the extras, fills required nulls, and coerces an enum only when the
//! lower or upper case form is one of its values (`COMMENTED` becomes `commented`; `OWNER` stays
//! `OWNER`).

use serde_json::{Map, Value};

use crate::validate::webhook_definition;

/// Projects `value` onto the webhook definition `name`. An unknown name is returned unchanged.
pub(crate) fn project(name: &str, value: Value) -> Value {
    match webhook_definition(name) {
        Some(schema) => apply(schema, value),
        None => value,
    }
}

/// Follows a `$ref` of the form `#/definitions/<name>` a few times.
fn resolve(schema: &Value) -> &Value {
    let mut current = schema;
    for _ in 0..12 {
        let Some(reference) = current.get("$ref").and_then(Value::as_str) else {
            return current;
        };
        let Some(name) = reference.rsplit('/').next() else {
            return current;
        };
        let Some(next) = webhook_definition(name) else {
            return current;
        };
        current = next;
    }
    current
}

fn apply(schema: &Value, value: Value) -> Value {
    let schema = resolve(schema);
    if let Some(branches) = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
        && !branches.is_empty()
    {
        let index = best_branch(branches, &value);
        return apply(&branches[index], value);
    }
    if let Some(parts) = schema.get("allOf").and_then(Value::as_array) {
        return parts.iter().fold(value, |value, part| apply(part, value));
    }
    if schema.get("type") == Some(&Value::from("null")) {
        return Value::Null;
    }
    let mut value = coerce_enum(schema, value);
    if value.as_str() == Some("")
        && matches!(
            schema.get("format").and_then(Value::as_str),
            Some("uri" | "uri-template")
        )
        && allows_null(schema)
    {
        value = Value::Null;
    }
    match schema.get("type") {
        Some(Value::String(kind)) if kind == "object" => apply_object(schema, value),
        Some(Value::String(kind)) if kind == "array" => apply_array(schema, value),
        _ if schema.get("properties").is_some() => apply_object(schema, value),
        _ => value,
    }
}

fn best_branch(branches: &[Value], value: &Value) -> usize {
    let mut best = 0;
    let mut best_score = i32::MIN;
    for (index, branch) in branches.iter().enumerate() {
        let score = score(branch, value);
        if score > best_score {
            best = index;
            best_score = score;
        }
    }
    best
}

fn score(schema: &Value, value: &Value) -> i32 {
    let schema = resolve(schema);
    if let Some(branches) = schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
    {
        return branches
            .iter()
            .map(|branch| score(branch, value))
            .max()
            .unwrap_or(0);
    }
    let mut total = 0;
    if let Some(kinds) = type_names(schema) {
        if kinds.iter().any(|kind| type_matches(kind, value)) {
            total += 8;
        } else if !kinds.is_empty() {
            total -= 30;
        }
    }
    if let Some(choices) = schema.get("enum").and_then(Value::as_array) {
        if choices.iter().any(|item| item == value) {
            total += 20;
        } else if let Some(text) = value.as_str() {
            let lower = text.to_ascii_lowercase();
            let upper = text.to_ascii_uppercase();
            if choices.iter().any(|item| {
                item.as_str() == Some(lower.as_str()) || item.as_str() == Some(upper.as_str())
            }) {
                total += 16;
            } else {
                total -= 40;
            }
        } else {
            total -= 40;
        }
    }
    match (
        schema.get("properties").and_then(Value::as_object),
        value.as_object(),
    ) {
        (Some(properties), Some(object)) => {
            for key in required_keys(schema) {
                match object.get(&key) {
                    Some(child) if !child.is_null() => total += 3,
                    Some(_) => total += 1,
                    None => total -= 2,
                }
            }
            if let (Some(status), Some(status_schema)) =
                (object.get("status"), properties.get("status"))
            {
                total += score(status_schema, status);
            }
        }
        (Some(_), None) => total -= 8,
        _ => {}
    }
    total
}

fn type_names(schema: &Value) -> Option<Vec<&str>> {
    match schema.get("type")? {
        Value::String(kind) => Some(vec![kind.as_str()]),
        Value::Array(kinds) => Some(kinds.iter().filter_map(Value::as_str).collect()),
        _ => None,
    }
}

fn type_matches(kind: &str, value: &Value) -> bool {
    match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "number" => value.is_number(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        _ => false,
    }
}

fn allows_null(schema: &Value) -> bool {
    let schema = resolve(schema);
    if type_names(schema).is_some_and(|kinds| kinds.contains(&"null")) {
        return true;
    }
    if schema
        .get("enum")
        .and_then(Value::as_array)
        .is_some_and(|choices| choices.iter().any(Value::is_null))
    {
        return true;
    }
    schema
        .get("oneOf")
        .or_else(|| schema.get("anyOf"))
        .and_then(Value::as_array)
        .is_some_and(|branches| branches.iter().any(allows_null))
}

fn coerce_enum(schema: &Value, value: Value) -> Value {
    let Some(choices) = schema.get("enum").and_then(Value::as_array) else {
        return value;
    };
    if choices.iter().any(|item| item == &value) {
        return value;
    }
    let Some(text) = value.as_str() else {
        return value;
    };
    let lower = Value::from(text.to_ascii_lowercase());
    if choices.iter().any(|item| item == &lower) {
        return lower;
    }
    let upper = Value::from(text.to_ascii_uppercase());
    if choices.iter().any(|item| item == &upper) {
        return upper;
    }
    value
}

fn required_keys(schema: &Value) -> Vec<String> {
    schema
        .get("required")
        .and_then(Value::as_array)
        .map(|keys| {
            keys.iter()
                .filter_map(|key| key.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default()
}

fn sole_type(schema: &Value) -> Option<&str> {
    match schema.get("type")? {
        Value::String(kind) => Some(kind.as_str()),
        _ => None,
    }
}

fn apply_object(schema: &Value, value: Value) -> Value {
    let mut input = match value {
        Value::Object(map) => map,
        other => return other,
    };
    let Some(properties) = schema.get("properties").and_then(Value::as_object) else {
        return Value::Object(input);
    };
    let required = required_keys(schema);
    let mut output = Map::new();
    for (key, property) in properties {
        let property = resolve(property);
        match input.remove(key) {
            Some(child) => {
                let mut projected = apply(property, child);
                if projected.is_null()
                    && !allows_null(property)
                    && sole_type(property) == Some("boolean")
                {
                    projected = Value::Bool(false);
                }
                if projected.is_null()
                    && !allows_null(property)
                    && !required.iter().any(|item| item == key)
                {
                    continue;
                }
                output.insert(key.clone(), projected);
            }
            None if required.iter().any(|item| item == key) => {
                if allows_null(property) {
                    output.insert(key.clone(), Value::Null);
                } else if sole_type(property) == Some("array") {
                    output.insert(key.clone(), Value::Array(Vec::new()));
                } else if sole_type(property) == Some("boolean") {
                    output.insert(key.clone(), Value::Bool(false));
                }
            }
            None => {}
        }
    }
    if schema.get("additionalProperties") == Some(&Value::Bool(false)) {
        return Value::Object(output);
    }
    if let Some(extra) = schema
        .get("additionalProperties")
        .filter(|value| value.is_object())
    {
        for (key, child) in input {
            output.insert(key, apply(extra, child));
        }
    } else {
        output.extend(input);
    }
    Value::Object(output)
}

fn apply_array(schema: &Value, value: Value) -> Value {
    let Value::Array(items) = value else {
        return value;
    };
    let Some(item_schema) = schema.get("items") else {
        return Value::Array(items);
    };
    Value::Array(
        items
            .into_iter()
            .map(|item| apply(item_schema, item))
            .collect(),
    )
}
