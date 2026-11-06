//! Checks the generated operation table against the pinned spec.

use std::collections::HashSet;

use ninot_github::{Method, OPERATIONS, Operation, ops};
use serde_json::Value;

#[test]
fn table_has_every_operation_in_the_spec() {
    let spec: Value =
        serde_json::from_slice(&std::fs::read("spec/api.github.com.json").unwrap()).unwrap();
    let expected: HashSet<&str> = spec["paths"]
        .as_object()
        .unwrap()
        .values()
        .flat_map(|item| item.as_object().unwrap().values())
        .filter_map(|operation| operation.get("operationId")?.as_str())
        .collect();
    let generated: HashSet<&str> = OPERATIONS.iter().map(|info| info.id).collect();
    assert_eq!(generated.len(), OPERATIONS.len(), "duplicate operationId");
    assert_eq!(generated, expected);
}

#[test]
fn operation_types_carry_their_route() {
    let info = <ops::ReposGet as Operation>::INFO;
    assert_eq!(info.id, "repos/get");
    assert_eq!(info.method, Method::Get);
    assert_eq!(info.path, "/repos/{owner}/{repo}");
}
