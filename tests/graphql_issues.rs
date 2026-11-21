//! GraphQL issue fields and mutations, over plain HTTP.

mod common;

use common::Api;
use ninot_github::Server;
use serde_json::{Value, json};

fn query(api: &Api, query: &str, variables: Value) -> Value {
    let reply = api.post(
        "/api/graphql",
        json!({ "query": query, "variables": variables }),
    );
    assert_eq!(reply.status, 200, "{}", reply.body);
    reply.body
}

fn repo(server: &Server) -> ninot_github::server::Repository {
    server
        .add_repository("admin", "tool", false, None, None, "main")
        .unwrap()
}

#[test]
fn repository_labels_paginate_and_filter() {
    let server = Server::start().unwrap();
    let repository = repo(&server);
    let admin = Api::admin(&server);
    for name in ["alpha", "beta", "gamma"] {
        admin.post(
            "/repos/admin/tool/labels",
            json!({ "name": name, "color": "ededed" }),
        );
    }
    let page = query(
        &admin,
        "query($after: String){repository(owner: \"admin\", name: \"tool\"){labels(first: 1, after: $after, orderBy: {field: NAME, direction: ASC}){totalCount nodes{name} pageInfo{hasNextPage endCursor}}}}",
        json!({ "after": null }),
    );
    assert!(page.get("errors").is_none(), "{page}");
    let labels = &page["data"]["repository"]["labels"];
    assert_eq!(labels["totalCount"], 3);
    assert_eq!(labels["nodes"][0]["name"], "alpha");
    assert_eq!(labels["pageInfo"]["hasNextPage"], true);
    let second = query(
        &admin,
        "query($after: String){repository(owner: \"admin\", name: \"tool\"){labels(first: 1, after: $after, orderBy: {field: NAME, direction: ASC}){nodes{name} pageInfo{hasNextPage}}}}",
        json!({ "after": labels["pageInfo"]["endCursor"] }),
    );
    assert_eq!(
        second["data"]["repository"]["labels"]["nodes"][0]["name"],
        "beta"
    );
    let searched = query(
        &admin,
        "{repository(owner: \"admin\", name: \"tool\"){labels(first: 10, query: \"amm\"){nodes{name}}}}",
        json!({}),
    );
    assert_eq!(
        searched["data"]["repository"]["labels"]["nodes"],
        json!([{ "name": "gamma" }])
    );
    let _ = repository;
}

#[test]
fn repository_issues_filter_and_paginate() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    repo(&server);
    admin.post("/repos/admin/tool/issues", json!({ "title": "One" }));
    admin.post("/repos/admin/tool/issues", json!({ "title": "Two" }));
    admin.patch("/repos/admin/tool/issues/1", json!({ "state": "closed" }));
    let page = query(
        &admin,
        "{repository(owner: \"admin\", name: \"tool\"){issues(first: 1, states: OPEN, orderBy: {field: CREATED_AT, direction: DESC}){totalCount nodes{number title state} pageInfo{hasNextPage endCursor}}}}",
        json!({}),
    );
    assert!(page.get("errors").is_none(), "{page}");
    let issues = &page["data"]["repository"]["issues"];
    assert_eq!(issues["totalCount"], 1);
    assert_eq!(issues["nodes"][0]["number"], 2);
    assert_eq!(issues["nodes"][0]["state"], "OPEN");
    let all = query(
        &admin,
        "{repository(owner: \"admin\", name: \"tool\"){issues(first: 10, states: [OPEN, CLOSED]){totalCount nodes{number}}}}",
        json!({}),
    );
    assert_eq!(all["data"]["repository"]["issues"]["totalCount"], 2);
}

#[test]
fn issue_or_pull_request_and_missing_issue() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    repo(&server);
    admin.post("/repos/admin/tool/issues", json!({ "title": "Bug" }));
    let found = query(
        &admin,
        "{repository(owner: \"admin\", name: \"tool\"){issueOrPullRequest(number: 1){__typename ... on Issue{number title id url locked isPinned stateReason}}}}",
        json!({}),
    );
    assert!(found.get("errors").is_none(), "{found}");
    let issue = &found["data"]["repository"]["issueOrPullRequest"];
    assert_eq!(issue["__typename"], "Issue");
    assert_eq!(issue["number"], 1);
    assert_eq!(issue["isPinned"], false);
    let missing = query(
        &admin,
        "{repository(owner: \"admin\", name: \"tool\"){issueOrPullRequest(number: 9){__typename}}}",
        json!({}),
    );
    assert_eq!(missing["errors"][0]["type"], "NOT_FOUND");
}

#[test]
fn create_issue_mutation_and_comment_url() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    repo(&server);
    admin.post(
        "/repos/admin/tool/labels",
        json!({ "name": "bug", "color": "d73a4a" }),
    );
    let repo_id = query(
        &admin,
        "{repository(owner: \"admin\", name: \"tool\"){id labels(first: 1){nodes{id}}}}",
        json!({}),
    );
    let id = repo_id["data"]["repository"]["id"].as_str().unwrap();
    let label = repo_id["data"]["repository"]["labels"]["nodes"][0]["id"]
        .as_str()
        .unwrap();
    let created = query(
        &admin,
        "mutation($input: CreateIssueInput!){createIssue(input: $input){issue{id url}}}",
        json!({ "input": { "repositoryId": id, "title": "Created", "body": "Via GQL", "labelIds": [label] } }),
    );
    assert!(created.get("errors").is_none(), "{created}");
    let issue_id = created["data"]["createIssue"]["issue"]["id"].clone();
    assert!(
        created["data"]["createIssue"]["issue"]["url"]
            .as_str()
            .unwrap()
            .ends_with("/admin/tool/issues/1")
    );
    let commented = query(
        &admin,
        "mutation($input: AddCommentInput!){addComment(input: $input){commentEdge{node{url}}}}",
        json!({ "input": { "subjectId": issue_id, "body": "Hi" } }),
    );
    assert!(commented.get("errors").is_none(), "{commented}");
    let url = commented["data"]["addComment"]["commentEdge"]["node"]["url"]
        .as_str()
        .unwrap();
    assert!(url.contains("issuecomment-"), "{url}");
}

#[test]
fn pin_lock_and_delete_mutations() {
    let server = Server::start().unwrap();
    let admin = Api::admin(&server);
    repo(&server);
    admin.post("/repos/admin/tool/issues", json!({ "title": "Pin me" }));
    let id = query(
        &admin,
        "{repository(owner: \"admin\", name: \"tool\"){issue(number: 1){id}}}",
        json!({}),
    )["data"]["repository"]["issue"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let pinned = query(
        &admin,
        "mutation($id: ID!){pinIssue(input: {issueId: $id}){issue{id}}}",
        json!({ "id": id }),
    );
    assert!(pinned.get("errors").is_none(), "{pinned}");
    let locked = query(
        &admin,
        "mutation($id: ID!){lockLockable(input: {lockableId: $id, lockReason: RESOLVED}){lockedRecord{locked}}}",
        json!({ "id": id }),
    );
    assert_eq!(
        locked["data"]["lockLockable"]["lockedRecord"]["locked"],
        true
    );
    let viewed = query(
        &admin,
        "{repository(owner: \"admin\", name: \"tool\"){issue(number: 1){isPinned locked activeLockReason comments(first: 10){totalCount pageInfo{hasNextPage}}}}}",
        json!({}),
    );
    let issue = &viewed["data"]["repository"]["issue"];
    assert_eq!(issue["isPinned"], true);
    assert_eq!(issue["locked"], true);
    assert_eq!(issue["activeLockReason"], "RESOLVED");
    assert_eq!(issue["comments"]["totalCount"], 0);
    query(
        &admin,
        "mutation($id: ID!){deleteIssue(input: {issueId: $id}){repository{id}}}",
        json!({ "id": id }),
    );
    let gone = admin.get("/repos/admin/tool/issues/1");
    assert_eq!(gone.status, 404);
}
