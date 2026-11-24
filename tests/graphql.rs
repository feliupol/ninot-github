//! The GraphQL subset, over plain HTTP.

mod common;

use common::Api;
use ninot_github::Server;
use ninot_github::fixtures::{
    IssueSeed, LabelSeed, PullRequestSeed, ReviewSeed, SeedField, UserSeed,
};
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
        .add_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let issue = repository
        .add_issue(
            7,
            IssueSeed {
                title: SeedField::present("Closed bug".to_owned()),
                state: SeedField::present("closed".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    let project = repository.add_project("Board").unwrap();
    project
        .add_field("Status", vec!["Todo".to_owned()])
        .unwrap();
    project
        .add_issue_item(&issue, [("Status".to_owned(), "Todo".to_owned())].into())
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
        .add_repository("admin", "project", false, None, None, "main")
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
        .add_repository("admin", "project", false, None, None, "main")
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

/// Seeds pull request 9 of `acme/project` with `reviews`, each `(login, state)`, in order.
/// alice and bob own `acme` and can push; carol cannot.
fn reviewed_pull(server: &Server, reviews: &[(&str, &str)]) {
    server.add_org("acme", &["alice", "bob"]);
    let repository = server
        .add_repository("acme", "project", false, None, None, "main")
        .unwrap();
    let pull = repository
        .add_pull_request(9, PullRequestSeed::default())
        .unwrap();
    for (login, state) in reviews {
        pull.add_review(ReviewSeed {
            state: SeedField::present((*state).to_owned()),
            user: SeedField::present(Some(UserSeed {
                login: SeedField::present((*login).to_owned()),
                ..Default::default()
            })),
            ..Default::default()
        })
        .unwrap();
    }
}

fn review_decision(server: &Server) -> Value {
    let reply = query(
        &Api::admin(server),
        "{repository(owner: \"acme\", name: \"project\"){pullRequest(number: 9){reviewDecision}}}",
        json!({}),
    );
    assert!(reply.get("errors").is_none(), "{reply}");
    reply["data"]["repository"]["pullRequest"]["reviewDecision"].clone()
}

#[test]
fn review_decision_counts_each_reviewers_latest_review() {
    let server = Server::start().unwrap();
    reviewed_pull(
        &server,
        &[("alice", "CHANGES_REQUESTED"), ("alice", "APPROVED")],
    );
    assert_eq!(review_decision(&server), "APPROVED");
}

#[test]
fn review_decision_is_changes_requested_while_any_reviewer_requests_changes() {
    let server = Server::start().unwrap();
    reviewed_pull(
        &server,
        &[("alice", "APPROVED"), ("bob", "CHANGES_REQUESTED")],
    );
    assert_eq!(review_decision(&server), "CHANGES_REQUESTED");
}

#[test]
fn review_decision_ignores_reviewers_who_cannot_push() {
    let server = Server::start().unwrap();
    reviewed_pull(
        &server,
        &[("carol", "APPROVED"), ("carol", "CHANGES_REQUESTED")],
    );
    assert_eq!(review_decision(&server), Value::Null);
}

#[test]
fn latest_opinionated_reviews_list_each_pushers_latest_opinion() {
    let server = Server::start().unwrap();
    reviewed_pull(
        &server,
        &[
            ("alice", "CHANGES_REQUESTED"),
            ("carol", "APPROVED"),
            ("bob", "COMMENTED"),
            ("alice", "APPROVED"),
            ("bob", "DISMISSED"),
        ],
    );
    let reply = query(
        &Api::admin(&server),
        "{repository(owner: \"acme\", name: \"project\"){pullRequest(number: 9){latestOpinionatedReviews(first: 10, writersOnly: false){totalCount nodes{state author{login} authorCanPushToRepository}}}}}",
        json!({}),
    );
    assert!(reply.get("errors").is_none(), "{reply}");
    assert_eq!(
        reply["data"]["repository"]["pullRequest"]["latestOpinionatedReviews"],
        json!({"totalCount": 2, "nodes": [
            {"state": "APPROVED", "author": {"login": "alice"}, "authorCanPushToRepository": true},
            {"state": "DISMISSED", "author": {"login": "bob"}, "authorCanPushToRepository": true},
        ]})
    );
}

#[test]
fn reviews_report_whether_their_author_can_push() {
    let server = Server::start().unwrap();
    reviewed_pull(&server, &[("alice", "APPROVED"), ("carol", "APPROVED")]);
    let reply = query(
        &Api::admin(&server),
        "{repository(owner: \"acme\", name: \"project\"){pullRequest(number: 9){reviews(first: 10){nodes{author{login} authorCanPushToRepository}}}}}",
        json!({}),
    );
    assert!(reply.get("errors").is_none(), "{reply}");
    assert_eq!(
        reply["data"]["repository"]["pullRequest"]["reviews"]["nodes"],
        json!([
            {"author": {"login": "alice"}, "authorCanPushToRepository": true},
            {"author": {"login": "carol"}, "authorCanPushToRepository": false},
        ])
    );
}

#[test]
fn review_decision_ignores_comments_after_an_approval() {
    let server = Server::start().unwrap();
    reviewed_pull(&server, &[("alice", "APPROVED"), ("alice", "COMMENTED")]);
    assert_eq!(review_decision(&server), "APPROVED");
}

#[test]
fn repository_pull_request_resolves_the_pull_requests_fields() {
    let server = Server::start().unwrap();
    let repository = server
        .add_repository("admin", "project", false, None, None, "main")
        .unwrap();
    repository
        .add_pull_request(
            9,
            PullRequestSeed {
                title: SeedField::present("Typed".to_owned()),
                draft: SeedField::present(true),
                labels: SeedField::present(vec![LabelSeed {
                    id: SeedField::present(1),
                    name: SeedField::present("bug".to_owned()),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        )
        .unwrap();
    let reply = query(
        &Api::admin(&server),
        "{repository(owner: \"admin\", name: \"project\"){pullRequest(number: 9){number title isDraft state labels(first: 10){totalCount nodes{name}} reviewRequests(first: 10){pageInfo{hasNextPage}}}}}",
        json!({}),
    );
    assert!(reply.get("errors").is_none(), "{reply}");
    assert_eq!(
        reply["data"]["repository"]["pullRequest"],
        json!({
            "number": 9,
            "title": "Typed",
            "isDraft": true,
            "state": "OPEN",
            "labels": {"totalCount": 1, "nodes": [{"name": "bug"}]},
            "reviewRequests": {"pageInfo": {"hasNextPage": false}},
        })
    );
}

#[test]
fn searched_issues_are_graphql_issues() {
    let server = Server::start().unwrap();
    let repository = server
        .add_repository("admin", "project", false, None, None, "main")
        .unwrap();
    repository
        .add_issue(
            7,
            IssueSeed {
                title: SeedField::present("No body".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    let reply = query(
        &Api::admin(&server),
        "{search(query: \"repo:admin/project\", type: ISSUE, first: 10){nodes{... on Issue{id number body state url labels(first: 10){totalCount}}}}}",
        json!({}),
    );
    assert!(reply.get("errors").is_none(), "{reply}");
    assert_eq!(
        reply["data"]["search"]["nodes"][0],
        json!({
            "id": "I_admin_project_7",
            "number": 7,
            "body": "",
            "state": "OPEN",
            "url": format!("{}/admin/project/issues/7", server.url()),
            "labels": {"totalCount": 0},
        })
    );
}

#[test]
fn node_resolves_project_items_with_field_values_and_content_labels() {
    let server = Server::start().unwrap();
    let repository = server
        .add_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let issue = repository
        .add_issue(
            3,
            IssueSeed {
                title: SeedField::present("Labelled".to_owned()),
                labels: SeedField::present(vec![LabelSeed {
                    name: SeedField::present("bug".to_owned()),
                    ..Default::default()
                }]),
                ..Default::default()
            },
        )
        .unwrap();
    let project = repository.add_project("Board").unwrap();
    project
        .add_field("Status", vec!["Todo".to_owned(), "Done".to_owned()])
        .unwrap();
    project
        .add_issue_item(&issue, [("Status".to_owned(), "Done".to_owned())].into())
        .unwrap();
    let reply = query(
        &Api::admin(&server),
        "query($id: ID!){node(id: $id){... on ProjectV2{title items(first: 10){totalCount nodes{id fieldValues(first: 10){nodes{... on ProjectV2ItemFieldSingleSelectValue{name field{... on ProjectV2SingleSelectField{name}}}}} content{... on Issue{number labels(first: 10){nodes{name}}}}}}}}}",
        json!({"id": project.node_id()}),
    );
    assert!(reply.get("errors").is_none(), "{reply}");
    let items = &reply["data"]["node"]["items"];
    assert_eq!(items["totalCount"], 1);
    assert_eq!(
        items["nodes"][0]["fieldValues"]["nodes"],
        json!([{"name": "Done", "field": {"name": "Status"}}])
    );
    assert_eq!(
        items["nodes"][0]["content"],
        json!({"number": 3, "labels": {"nodes": [{"name": "bug"}]}})
    );
}

#[test]
fn add_project_item_by_id_adds_an_issue_once() {
    let server = Server::start().unwrap();
    let repository = server
        .add_repository("admin", "project", false, None, None, "main")
        .unwrap();
    repository
        .add_issue(
            4,
            IssueSeed {
                title: SeedField::present("Plan".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    let project = repository.add_project("Board").unwrap();
    let admin = Api::admin(&server);
    let add = |content: &str| {
        query(
            &admin,
            "mutation($input: AddProjectV2ItemByIdInput!){addProjectV2ItemById(input: $input){item{id type content{... on Issue{number title}}}}}",
            json!({"input": {"projectId": project.node_id(), "contentId": content}}),
        )
    };
    let first = add("I_admin_project_4");
    assert!(first.get("errors").is_none(), "{first}");
    let item = &first["data"]["addProjectV2ItemById"]["item"];
    assert_eq!(item["type"], "ISSUE");
    assert_eq!(item["content"], json!({"number": 4, "title": "Plan"}));
    let again = add("I_admin_project_4");
    assert_eq!(
        again["data"]["addProjectV2ItemById"]["item"]["id"],
        item["id"]
    );

    let listed = query(
        &admin,
        "query($id: ID!){node(id: $id){... on ProjectV2{items(first: 10){totalCount}}}}",
        json!({"id": project.node_id()}),
    );
    assert_eq!(listed["data"]["node"]["items"]["totalCount"], 1);

    let missing = add("I_admin_project_99");
    assert_eq!(missing["errors"][0]["type"], "NOT_FOUND");
    let bad_project = query(
        &admin,
        "mutation($input: AddProjectV2ItemByIdInput!){addProjectV2ItemById(input: $input){item{id}}}",
        json!({"input": {"projectId": "PVT_x", "contentId": "I_admin_project_4"}}),
    );
    assert_eq!(bad_project["errors"][0]["type"], "NOT_FOUND");
}

#[test]
fn pull_requests_resolve_review_threads() {
    let server = Server::start().unwrap();
    server.add_org("acme", &["admin"]);
    let repository = server
        .add_repository("acme", "project", false, None, None, "main")
        .unwrap();
    let pull = repository
        .add_pull_request(
            9,
            PullRequestSeed {
                title: SeedField::present("Change".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    let thread = pull
        .add_review_thread(ninot_github::fixtures::ReviewThreadSeed {
            is_resolved: SeedField::present(true),
            path: SeedField::present(Some("src/lib.rs".to_owned())),
            line: SeedField::present(Some(12)),
            comments: SeedField::present(vec![ninot_github::fixtures::ReviewThreadCommentSeed {
                body: SeedField::present("Rename this".to_owned()),
                ..Default::default()
            }]),
            ..Default::default()
        })
        .unwrap();
    let reply = query(
        &Api::admin(&server),
        "{repository(owner: \"acme\", name: \"project\"){pullRequest(number: 9){reviewThreads(first: 10){totalCount nodes{id isResolved path line comments(first: 10){nodes{body}}}}}}}",
        json!({}),
    );
    assert!(reply.get("errors").is_none(), "{reply}");
    let threads = &reply["data"]["repository"]["pullRequest"]["reviewThreads"];
    assert_eq!(threads["totalCount"], 1);
    assert_eq!(
        threads["nodes"][0],
        json!({
            "id": thread,
            "isResolved": true,
            "path": "src/lib.rs",
            "line": 12,
            "comments": {"nodes": [{"body": "Rename this"}]}
        })
    );
}

#[test]
fn last_pages_report_the_items_before_them() {
    let server = Server::start().unwrap();
    reviewed_pull(
        &server,
        &[
            ("alice", "COMMENTED"),
            ("bob", "APPROVED"),
            ("alice", "APPROVED"),
        ],
    );
    let admin = Api::admin(&server);
    let reviews = |args: &str| {
        let reply = query(
            &admin,
            &format!(
                "{{repository(owner: \"acme\", name: \"project\"){{pullRequest(number: 9){{reviews({args}){{totalCount nodes{{state author{{login}}}} pageInfo{{hasPreviousPage hasNextPage}}}}}}}}}}"
            ),
            json!({}),
        );
        assert!(reply.get("errors").is_none(), "{reply}");
        reply["data"]["repository"]["pullRequest"]["reviews"].clone()
    };
    let last = reviews("last: 2");
    assert_eq!(last["totalCount"], 3);
    assert_eq!(
        last["nodes"],
        json!([
            {"state": "APPROVED", "author": {"login": "bob"}},
            {"state": "APPROVED", "author": {"login": "alice"}}
        ])
    );
    assert_eq!(
        last["pageInfo"],
        json!({"hasPreviousPage": true, "hasNextPage": false})
    );
    let all = reviews("last: 10");
    assert_eq!(all["nodes"].as_array().unwrap().len(), 3);
    assert_eq!(all["pageInfo"]["hasPreviousPage"], false);
    let first = reviews("first: 1");
    assert_eq!(
        first["pageInfo"],
        json!({"hasPreviousPage": false, "hasNextPage": true})
    );
}

#[test]
fn last_takes_the_final_comments_of_an_issue() {
    let server = Server::start().unwrap();
    let repository = server
        .add_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let issue = repository
        .add_issue(
            1,
            IssueSeed {
                title: SeedField::present("Talk".to_owned()),
                ..Default::default()
            },
        )
        .unwrap();
    for body in ["one", "two", "three"] {
        issue
            .add_comment(ninot_github::fixtures::IssueCommentSeed {
                body: SeedField::present(body.to_owned()),
                ..Default::default()
            })
            .unwrap();
    }
    let reply = query(
        &Api::admin(&server),
        "{repository(owner: \"admin\", name: \"project\"){issue(number: 1){comments(last: 2){nodes{body} pageInfo{hasPreviousPage hasNextPage}}}}}",
        json!({}),
    );
    assert!(reply.get("errors").is_none(), "{reply}");
    let comments = &reply["data"]["repository"]["issue"]["comments"];
    assert_eq!(
        comments["nodes"],
        json!([{"body": "two"}, {"body": "three"}])
    );
    assert_eq!(
        comments["pageInfo"],
        json!({"hasPreviousPage": true, "hasNextPage": false})
    );
}

#[test]
fn pull_comments_carry_edit_times_and_page_backwards() {
    let server = Server::start().unwrap();
    let repository = server
        .add_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let pull = repository
        .add_pull_request(9, PullRequestSeed::default())
        .unwrap();
    for body in ["one", "two", "three"] {
        pull.add_issue_comment(ninot_github::fixtures::IssueCommentSeed {
            body: SeedField::present(body.to_owned()),
            created_at: SeedField::present("2026-01-01T00:00:00Z".to_owned()),
            updated_at: SeedField::present("2026-01-01T00:00:00Z".to_owned()),
            ..Default::default()
        })
        .unwrap();
    }
    let api = Api::admin(&server);
    let fields = "id createdAt updatedAt lastEditedAt author{login} body";
    let read = |args: &str| {
        let reply = query(
            &api,
            &format!(
                "{{repository(owner: \"admin\", name: \"project\"){{pullRequest(number: 9){{comments({args}){{nodes{{{fields}}} pageInfo{{hasPreviousPage hasNextPage startCursor}}}}}}}}}}"
            ),
            json!({}),
        );
        assert!(reply.get("errors").is_none(), "{reply}");
        reply["data"]["repository"]["pullRequest"]["comments"].clone()
    };
    let last = read("last: 2");
    let nodes = last["nodes"].as_array().unwrap();
    assert_eq!(nodes[0]["body"], "two");
    assert_eq!(nodes[0]["createdAt"], "2026-01-01T00:00:00Z");
    assert_eq!(nodes[0]["updatedAt"], "2026-01-01T00:00:00Z");
    assert_eq!(nodes[0]["lastEditedAt"], Value::Null);
    assert!(nodes[0]["id"].as_str().is_some_and(|id| !id.is_empty()));
    let before = read(&format!(
        "last: 1, before: \"{}\"",
        last["pageInfo"]["startCursor"].as_str().unwrap()
    ));
    assert_eq!(before["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(before["nodes"][0]["body"], "one");
    assert_eq!(before["pageInfo"]["hasPreviousPage"], false);
    assert_eq!(before["pageInfo"]["hasNextPage"], true);

    let id = nodes[0]["id"].as_str().unwrap();
    assert!(!id.is_empty());
    let listed = api.get("/repos/admin/project/issues/9/comments");
    let database_id = listed.body[1]["id"].as_u64().unwrap();
    let edited = api.patch(
        &format!("/repos/admin/project/issues/comments/{database_id}"),
        json!({"body": "two!"}),
    );
    assert_eq!(edited.status, 200, "{}", edited.body);
    let now = read("last: 2");
    let node = &now["nodes"][0];
    assert_eq!(node["body"], "two!");
    assert_ne!(node["updatedAt"], "2026-01-01T00:00:00Z");
    assert_eq!(node["lastEditedAt"], node["updatedAt"]);
}

#[test]
fn issue_comments_carry_edit_times_and_page_backwards() {
    let server = Server::start().unwrap();
    let repository = server
        .add_repository("admin", "project", false, None, None, "main")
        .unwrap();
    let issue = repository.add_issue(1, IssueSeed::default()).unwrap();
    for body in ["one", "two", "three"] {
        issue
            .add_comment(ninot_github::fixtures::IssueCommentSeed {
                body: SeedField::present(body.to_owned()),
                ..Default::default()
            })
            .unwrap();
    }
    let reply = query(
        &Api::admin(&server),
        "{repository(owner: \"admin\", name: \"project\"){issue(number: 1){comments(last: 2){pageInfo{startCursor}}}}}",
        json!({}),
    );
    let cursor = reply["data"]["repository"]["issue"]["comments"]["pageInfo"]["startCursor"]
        .as_str()
        .unwrap()
        .to_owned();
    let reply = query(
        &Api::admin(&server),
        &format!(
            "{{repository(owner: \"admin\", name: \"project\"){{issue(number: 1){{comments(last: 5, before: \"{cursor}\"){{nodes{{body updatedAt lastEditedAt}} pageInfo{{hasPreviousPage}}}}}}}}}}"
        ),
        json!({}),
    );
    assert!(reply.get("errors").is_none(), "{reply}");
    let comments = &reply["data"]["repository"]["issue"]["comments"];
    assert_eq!(comments["nodes"].as_array().unwrap().len(), 1);
    assert_eq!(comments["nodes"][0]["body"], "one");
    assert_eq!(comments["nodes"][0]["lastEditedAt"], Value::Null);
    assert!(comments["nodes"][0]["updatedAt"].is_string());
    assert_eq!(comments["pageInfo"]["hasPreviousPage"], false);
}
