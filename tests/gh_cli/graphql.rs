//! GraphQL documents that `gh` itself sends, posted through `gh api graphql`.

use std::path::Path;

use ninot_github::Server;

use crate::common::TempDir;
use crate::{api, gh, gh_json, open_pull, repo_with_branches, tls_server};

/// The GraphQL id of pull request `number` of `admin/tool`, as `gh pr view` reads it.
fn pull_id(server: &Server, config: &Path, number: &str) -> String {
    gh(
        server,
        config,
        &[
            "pr",
            "view",
            number,
            "-R",
            "admin/tool",
            "--json",
            "id",
            "--jq",
            ".id",
        ],
    )
    .trim()
    .to_owned()
}

/// Posts `query` with string `variables` given as `name=value`.
fn graphql(server: &Server, config: &Path, query: &str, variables: &[&str]) -> serde_json::Value {
    let mut args = vec![
        "api".to_owned(),
        "graphql".to_owned(),
        "-f".to_owned(),
        format!("query={query}"),
    ];
    for variable in variables {
        args.push("-f".to_owned());
        args.push((*variable).to_owned());
    }
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    gh_json(server, config, &args)
}

/// `GitHubRepo` of cli/cli v2.101.0 api/queries_repo.go:361.
#[test]
fn repository_info_query() {
    let server = tls_server();
    let config = TempDir::new();
    gh(
        &server,
        &config.0,
        &[
            "repo",
            "create",
            "admin/tool",
            "--public",
            "--add-readme",
            "-d",
            "A tool",
        ],
    );

    let reply = graphql(
        &server,
        &config.0,
        "fragment repo on Repository { id databaseId name owner { login } sshUrl \
         hasIssuesEnabled description hasWikiEnabled viewerPermission defaultBranchRef { name } } \
         query RepositoryInfo($owner: String!, $name: String!) { repository(owner: $owner, name: \
         $name) { ...repo parent { ...repo } mergeCommitAllowed rebaseMergeAllowed \
         squashMergeAllowed } }",
        &["owner=admin", "name=tool"],
    );
    let repository = &reply["data"]["repository"];
    assert_eq!(repository["name"], "tool");
    assert_eq!(repository["owner"]["login"], "admin");
    assert_eq!(repository["description"], "A tool");
    assert_eq!(repository["viewerPermission"], "ADMIN");
    assert_eq!(repository["defaultBranchRef"]["name"], "main");
    assert_eq!(repository["parent"], serde_json::Value::Null);
    assert_eq!(repository["mergeCommitAllowed"], true);
}

/// `PullRequestReadyForReview` and `ConvertPullRequestToDraft` of cli/cli v2.101.0
/// api/queries_pr.go:804 and :853.
#[test]
fn ready_for_review_and_convert_to_draft_mutations() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature"]);
    let number = open_pull(&server, &config.0, "feature", true);
    let id = pull_id(&server, &config.0, &number);
    let input = format!("input[pullRequestId]={id}");

    let ready = graphql(
        &server,
        &config.0,
        "mutation PullRequestReadyForReview($input:MarkPullRequestReadyForReviewInput!)\
         {markPullRequestReadyForReview(input: $input){pullRequest{id}}}",
        &[&input],
    );
    assert_eq!(
        ready["data"]["markPullRequestReadyForReview"]["pullRequest"]["id"],
        id
    );
    assert_eq!(
        api(&server, &config.0, "repos/admin/tool/pulls/1", ".draft"),
        "false"
    );

    graphql(
        &server,
        &config.0,
        "mutation ConvertPullRequestToDraft($input:ConvertPullRequestToDraftInput!)\
         {convertPullRequestToDraft(input: $input){pullRequest{id}}}",
        &[&input],
    );
    assert_eq!(
        api(&server, &config.0, "repos/admin/tool/pulls/1", ".draft"),
        "true"
    );
}

/// `PullRequestAutoMerge` and `PullRequestAutoMergeDisable` of cli/cli v2.101.0
/// pkg/cmd/pr/merge/http.go:89 and :106.
#[test]
#[ignore = "not implemented: GraphQL EnablePullRequestAutoMergePayload.clientMutationId"]
fn auto_merge_mutations() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature"]);
    gh(
        &server,
        &config.0,
        &["repo", "edit", "admin/tool", "--enable-auto-merge"],
    );
    let number = open_pull(&server, &config.0, "feature", false);
    let id = pull_id(&server, &config.0, &number);

    graphql(
        &server,
        &config.0,
        "mutation PullRequestAutoMerge($input:EnablePullRequestAutoMergeInput!)\
         {enablePullRequestAutoMerge(input: $input){clientMutationId}}",
        &[
            &format!("input[pullRequestId]={id}"),
            "input[mergeMethod]=SQUASH",
        ],
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/pulls/1",
            ".auto_merge.merge_method"
        ),
        "squash"
    );

    graphql(
        &server,
        &config.0,
        "mutation PullRequestAutoMergeDisable($prID:ID!)\
         {disablePullRequestAutoMerge(input: {pullRequestId: $prID}){clientMutationId}}",
        &[&format!("prID={id}")],
    );
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/pulls/1",
            ".auto_merge"
        ),
        "null"
    );
}

/// `CommentCreate` of cli/cli v2.101.0 api/queries_comments.go:55.
#[test]
fn comment_create_mutation() {
    let server = tls_server();
    let config = TempDir::new();
    repo_with_branches(&server, &config.0, &["feature"]);
    let number = open_pull(&server, &config.0, "feature", false);
    let id = pull_id(&server, &config.0, &number);

    let reply = graphql(
        &server,
        &config.0,
        "mutation CommentCreate($input:AddCommentInput!)\
         {addComment(input: $input){commentEdge{node{url}}}}",
        &[&format!("input[subjectId]={id}"), "input[body]=Looks fine."],
    );
    let url = reply["data"]["addComment"]["commentEdge"]["node"]["url"]
        .as_str()
        .unwrap();
    assert!(url.contains("/admin/tool/pull/1#issuecomment-"), "{url}");
    assert_eq!(
        api(
            &server,
            &config.0,
            "repos/admin/tool/issues/1/comments",
            ".[].body"
        ),
        "Looks fine."
    );
}
