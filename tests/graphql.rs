//! The GraphQL subset, over plain HTTP.

mod common;

use common::Api;
use ninot_github::Server;
use ninot_github::fixtures::{IssueSeed, SeedField};
use serde_json::{Value, json};

fn query(api: &Api, query: &str, variables: Value) -> Value {
    let reply = api.post(
        "/api/graphql",
        json!({ "query": query, "variables": variables }),
    );
    assert_eq!(reply.status, 200, "{}", reply.body);
    reply.body
}

#[test]
fn requires_authentication() {
    let server = Server::start().unwrap();
    let reply = Api::new(&server, None).post("/api/graphql", json!({ "query": "{viewer{login}}" }));
    assert_eq!(reply.status, 401);
}

#[test]
fn create_view_and_list_repositories() {
    let server = Server::start().unwrap();
    server.add_org("acme", &["admin"]);
    let admin = Api::admin(&server);
    let org_id = query(&admin, "{organization(login: \"acme\"){id}}", json!({}))["data"]["organization"]["id"].clone();
    let created = query(
        &admin,
        "mutation($input: CreateRepositoryInput!){createRepository(input: $input){repository{nameWithOwner isPrivate owner{login}}}}",
        json!({ "input": { "name": "tool", "visibility": "PRIVATE", "ownerId": org_id } }),
    );
    assert_eq!(
        created["data"]["createRepository"]["repository"],
        json!({ "nameWithOwner": "acme/tool", "isPrivate": true, "owner": { "login": "acme" } })
    );
    for name in ["b", "c"] {
        query(
            &admin,
            "mutation($input: CreateRepositoryInput!){createRepository(input: $input){repository{id}}}",
            json!({ "input": { "name": name, "visibility": "PUBLIC" } }),
        );
    }

    let page = |after: Value| {
        query(
            &admin,
            "query($after: String){repositoryOwner(login: \"admin\"){repositories(first: 1, after: $after, ownerAffiliations: OWNER, orderBy: {field: NAME, direction: ASC}){nodes{name} totalCount pageInfo{hasNextPage endCursor}}}}",
            json!({ "after": after }),
        )["data"]["repositoryOwner"]["repositories"]
            .clone()
    };
    let first = page(Value::Null);
    assert_eq!(first["nodes"], json!([{ "name": "b" }]));
    assert_eq!(first["totalCount"], 2);
    assert_eq!(first["pageInfo"]["hasNextPage"], true);
    let second = page(first["pageInfo"]["endCursor"].clone());
    assert_eq!(second["nodes"], json!([{ "name": "c" }]));
    assert_eq!(second["pageInfo"]["hasNextPage"], false);
}

#[test]
fn missing_repositories_are_not_found_errors() {
    let server = Server::start().unwrap();
    server.add_user("bob");
    let bob = Api::new(&server, Some(&server.add_user("bob").add_token(&["repo"])));
    Api::admin(&server).post("/user/repos", json!({ "name": "secret", "private": true }));
    let reply = query(
        &bob,
        "{repository(owner: \"admin\", name: \"secret\"){id}}",
        json!({}),
    );
    assert_eq!(reply["errors"][0]["type"], "NOT_FOUND");
    assert_eq!(
        reply["errors"][0]["message"],
        "Could not resolve to a Repository with the name 'admin/secret'."
    );
}

#[test]
fn introspection_lists_repository_fields() {
    let server = Server::start().unwrap();
    let reply = query(
        &Api::admin(&server),
        "{__type(name: \"Repository\"){fields{name}}}",
        json!({}),
    );
    let fields: Vec<&str> = reply["data"]["__type"]["fields"]
        .as_array()
        .unwrap()
        .iter()
        .map(|field| field["name"].as_str().unwrap())
        .collect();
    assert!(
        fields.contains(&"nameWithOwner") && fields.contains(&"sshUrl"),
        "{fields:?}"
    );
}

#[test]
fn introspection_serves_githubs_schema() {
    let server = Server::start().unwrap();
    let reply = query(
        &Api::admin(&server),
        "{__type(name: \"PullRequest\"){kind fields{name} interfaces{name}}}",
        json!({}),
    );
    let pull_request = &reply["data"]["__type"];
    assert_eq!(pull_request["kind"], "OBJECT", "{reply}");
    let names = |list: &Value| -> Vec<String> {
        list.as_array()
            .unwrap()
            .iter()
            .map(|item| item["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let fields = names(&pull_request["fields"]);
    for field in [
        "mergeable",
        "headRefName",
        "reviewDecision",
        "timelineItems",
    ] {
        assert!(
            fields.iter().any(|name| name == field),
            "{field} in {fields:?}"
        );
    }
    assert!(names(&pull_request["interfaces"]).contains(&"Node".to_owned()));
}

#[test]
fn unimplemented_fields_are_errors() {
    let server = Server::start().unwrap();
    let reply = query(&Api::admin(&server), "{viewer{login bio}}", json!({}));
    assert_eq!(
        reply["errors"][0]["message"], "Not implemented by ninot-github: User.bio",
        "{reply}"
    );
    assert_eq!(reply["errors"][0]["path"], json!(["viewer", "bio"]));
    assert_eq!(reply["data"]["viewer"]["login"], "admin");
}

#[test]
fn project_items_carry_issue_state_and_paged_comments() {
    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let issue = repository
        .create_issue(
            7,
            IssueSeed {
                title: SeedField::present("Closed bug".to_owned()),
                state: SeedField::present("closed".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    let project = repository.create_project("Board").unwrap();
    project
        .create_field("Status", vec!["Todo".to_owned()])
        .unwrap();
    project
        .create_issue_item(&issue, [("Status".to_owned(), "Todo".to_owned())].into())
        .unwrap();
    let reply = query(
        &Api::admin(&server),
        "query($id: ID!){node(id: $id){... on ProjectV2{items(first: 10){nodes{content{... on Issue{id number state comments(first: 100){totalCount nodes{body} pageInfo{hasNextPage endCursor}}}}}}}}}",
        json!({"id": project.node_id()}),
    );
    assert!(reply.get("errors").is_none(), "{reply}");
    let issue = &reply["data"]["node"]["items"]["nodes"][0]["content"];
    assert_eq!(issue["number"], 7);
    assert_eq!(issue["state"], "CLOSED");
    assert_eq!(issue["id"], "I_admin_project_7");
    assert_eq!(
        issue["comments"],
        json!({"totalCount": 0, "nodes": [], "pageInfo": {"hasNextPage": false, "endCursor": null}})
    );
}

#[test]
fn pull_request_commits_are_dated_by_their_commit_not_by_edits() {
    use std::path::Path;

    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let mut head = String::new();
    for branch in ["main", "feature"] {
        head = repository
            .commit_file(
                branch,
                Path::new(&format!("{branch}.txt")),
                b"content\n",
                "Add file",
            )
            .unwrap()
            .sha()
            .to_owned();
    }
    let admin = Api::admin(&server);
    let created = admin.post(
        "/repos/admin/project/pulls",
        json!({"title": "Dated", "head": "feature", "base": "main"}),
    );
    assert_eq!(created.status, 201, "{:?}", created.body);
    let committed = admin.get(&format!("/repos/admin/project/commits/{head}"));
    let commit_date = |reply: &Value| {
        let node = &reply["data"]["node"]["commits"]["nodes"][0]["commit"];
        assert_eq!(node["oid"], head, "{reply}");
        assert_eq!(node["parents"]["totalCount"], 1, "{reply}");
        chrono::DateTime::parse_from_rfc3339(node["committedDate"].as_str().unwrap()).unwrap()
    };
    let read = || {
        query(
            &admin,
            "query($id: ID!){node(id: $id){... on PullRequest{commits(last: 1){nodes{commit{oid committedDate parents{totalCount}}}}}}}",
            json!({"id": created.body["node_id"]}),
        )
    };
    let before = commit_date(&read());
    assert_eq!(
        before,
        chrono::DateTime::parse_from_rfc3339(
            committed.body["commit"]["committer"]["date"]
                .as_str()
                .unwrap()
        )
        .unwrap()
    );

    std::thread::sleep(std::time::Duration::from_millis(1100));
    let edited = admin.patch(
        &format!("/repos/admin/project/pulls/{}", created.body["number"]),
        json!({"title": "Renamed"}),
    );
    assert_eq!(edited.status, 200, "{:?}", edited.body);
    assert_eq!(commit_date(&read()), before);
}

#[test]
fn searched_pull_requests_resolve_their_repository_fields() {
    let server = Server::start().unwrap();
    let repository = server
        .create_repository("admin", "project", false, None, None, "main")
        .unwrap();
    repository.set_auto_merge_allowed(true).unwrap();
    for branch in ["main", "feature"] {
        repository
            .commit_file(
                branch,
                std::path::Path::new(&format!("{branch}.txt")),
                b"x\n",
                "Add file",
            )
            .unwrap();
    }
    let admin = Api::admin(&server);
    let created = admin.post(
        "/repos/admin/project/pulls",
        json!({"title": "Searched", "head": "feature", "base": "main"}),
    );
    assert_eq!(created.status, 201, "{:?}", created.body);

    let reply = query(
        &admin,
        "{search(query: \"is:pr repo:admin/project\", type: ISSUE, first: 10){nodes{... on PullRequest{repository{nameWithOwner autoMergeAllowed viewerPermission}}}}}",
        json!({}),
    );

    assert!(reply.get("errors").is_none(), "{reply}");
    assert_eq!(
        reply["data"]["search"]["nodes"][0]["repository"],
        json!({"nameWithOwner": "admin/project", "autoMergeAllowed": true, "viewerPermission": "ADMIN"})
    );
}
