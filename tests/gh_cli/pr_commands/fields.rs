//! GraphQL pull-request fields, pagination, and check rollup.

use ninot_github::fixtures::{CheckRunSeed, CommitStatusSeed, SeedField};

use super::graphql;
use crate::common::TempDir;
use crate::{api, gh, open_pull, repo_with_branches, tls_server};

#[test]
fn pr_fields_paginate_and_agree_with_rest() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature", "other"]);
    open_pull(&server, &config.0, "feature", false);
    open_pull(&server, &config.0, "other", false);
    let head = api(&server, &config.0, "repos/admin/tool/pulls/1", ".head.sha");
    server
        .repository("admin", "tool")
        .unwrap()
        .commit(&head)
        .unwrap()
        .create_status(CommitStatusSeed {
            state: SeedField::present("success".to_owned()),
            context: SeedField::present("tests".to_owned()),
            ..Default::default()
        })
        .unwrap();
    server
        .repository("admin", "tool")
        .unwrap()
        .commit(&head)
        .unwrap()
        .create_check_run(CheckRunSeed {
            name: SeedField::present("lint".to_owned()),
            status: SeedField::present("completed".to_owned()),
            conclusion: SeedField::present(Some("failure".to_owned())),
            ..Default::default()
        })
        .unwrap();
    gh(
        &server,
        &config.0,
        &[
            "api",
            "-X",
            "PUT",
            "repos/admin/tool/contents/later.txt",
            "-f",
            "message=Later",
            "-f",
            "content=bGF0ZXIK",
            "-f",
            "branch=main",
            "--silent",
        ],
    );

    let page = graphql(
        &server,
        &config.0,
        "query { repository(owner:\"admin\", name:\"tool\") { pullRequests(first:1, states:[OPEN]) \
         { totalCount pageInfo { hasNextPage endCursor } nodes { number } \
           edges { cursor node { number } } } } }",
    );
    let connection = &page["data"]["repository"]["pullRequests"];
    assert_eq!(connection["totalCount"], 2);
    assert_eq!(connection["pageInfo"]["hasNextPage"], true);
    assert_eq!(connection["nodes"].as_array().map(Vec::len), Some(1));
    assert_eq!(
        connection["edges"][0]["node"]["number"],
        connection["nodes"][0]["number"]
    );
    assert_eq!(
        connection["edges"][0]["cursor"],
        connection["pageInfo"]["endCursor"]
    );
    let cursor = connection["pageInfo"]["endCursor"].as_str().unwrap();
    let next = graphql(
        &server,
        &config.0,
        &format!(
            "query {{ repository(owner:\"admin\", name:\"tool\") {{ pullRequests(first:1, after:\"{cursor}\", \
             states:[OPEN]) {{ nodes {{ number }} pageInfo {{ hasNextPage }} }} }} }}"
        ),
    );
    let second = &next["data"]["repository"]["pullRequests"];
    assert_eq!(second["pageInfo"]["hasNextPage"], false);
    assert_ne!(
        second["nodes"][0]["number"],
        connection["nodes"][0]["number"]
    );

    let detail = graphql(
        &server,
        &config.0,
        "query { repository(owner:\"admin\", name:\"tool\") { pullRequest(number:1) { \
         state isDraft merged mergeable mergeStateStatus url headRefName baseRefName headRefOid \
         author { login ... on User { id databaseId } } \
         commits(last:1) { totalCount nodes { commit { oid messageHeadline } } } \
         files(first:1) { totalCount pageInfo { hasNextPage } nodes { path additions } } \
         viewerMergeHeadlineText(mergeType: MERGE) viewerMergeBodyText(mergeType: SQUASH) \
         baseRef { name compare(headRef: \"feature\") { behindBy aheadBy status } } \
         statusCheckRollup { state contexts(first:10) { totalCount \
           nodes { __typename ... on StatusContext { context state isRequired } \
             ... on CheckRun { name status conclusion isRequired } } } } } } }",
    );
    let pull = &detail["data"]["repository"]["pullRequest"];
    assert_eq!(pull["state"], "OPEN");
    assert_eq!(pull["isDraft"], false);
    assert_eq!(pull["merged"], false);
    assert_eq!(pull["mergeable"], "MERGEABLE");
    assert_eq!(pull["headRefName"], "feature");
    assert_eq!(pull["baseRefName"], "main");
    assert_eq!(pull["headRefOid"], head);
    assert_eq!(pull["author"]["login"], "admin");
    assert!(
        pull["author"]["id"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    assert!(pull["author"]["databaseId"].as_u64().is_some());
    assert!(pull["url"].as_str().unwrap().contains("/admin/tool/pull/1"));
    assert_eq!(pull["commits"]["nodes"][0]["commit"]["oid"], head);
    assert_eq!(pull["files"]["nodes"][0]["path"], "feature.txt");
    assert!(
        pull["viewerMergeHeadlineText"]
            .as_str()
            .unwrap()
            .contains("Merge pull request #1")
    );
    assert!(pull["baseRef"]["compare"]["behindBy"].as_u64().unwrap() >= 1);
    let contexts = pull["statusCheckRollup"]["contexts"]["nodes"]
        .as_array()
        .unwrap();
    assert!(
        contexts
            .iter()
            .any(|node| node["__typename"] == "StatusContext" && node["context"] == "tests")
    );
    assert!(
        contexts
            .iter()
            .any(|node| node["__typename"] == "CheckRun" && node["name"] == "lint")
    );
    assert_eq!(contexts[0]["isRequired"], false);
}
