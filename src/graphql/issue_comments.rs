//! `Mutation.addComment` for issues and pull requests.

use async_graphql::Error;
use serde::Deserialize;

use super::issue_actions::webhook_err;
use super::issues::issue_location as locate_issue;
use super::pulls::pull_id;
use super::{Env, Map, MutationArgs, Output, not_found, read, typed_error};
use crate::handlers::repos::Found;
use crate::render::node_id;
use crate::state::{Comment, Name, lock};
use crate::webhooks::events;

pub(super) const RESOLVERS: &[(&str, super::Resolver)] = &[("Mutation.addComment", add_comment)];

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AddCommentInput {
    subject_id: String,
    body: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct AddCommentPayload {
    #[serde(rename = "__typename")]
    typename: &'static str,
    comment_edge: CommentEdge,
}

#[derive(serde::Serialize)]
struct CommentEdge {
    node: CommentNode,
}

#[derive(serde::Serialize)]
struct CommentNode {
    #[serde(rename = "__typename")]
    typename: &'static str,
    id: String,
    url: String,
}

fn add_comment(env: &Env, _: &Map, args: &Map) -> Result<Output, Error> {
    let MutationArgs { input } = read::<MutationArgs<AddCommentInput>>(args)?;
    let mut state = lock(&env.state);
    if let Some((owner, name, number)) = locate_issue(&state, &input.subject_id) {
        return add_issue_comment(env, &mut state, owner, name, number, input.body);
    }
    add_pull_comment(&mut state, &input.subject_id, input.body)
}

fn add_issue_comment(
    env: &Env,
    state: &mut crate::state::State,
    owner: String,
    name: String,
    number: u64,
    body: String,
) -> Result<Output, Error> {
    let permission = state
        .permission(Some(&env.caller), &owner, &name)
        .ok_or_else(|| typed_error("Resource not accessible by integration", "FORBIDDEN"))?;
    let found = Found {
        owner: owner.clone(),
        name: name.clone(),
        permission,
    };
    let request = env.request();
    let id = state.next_id();
    let now = crate::time::now_utc();
    let record = Comment {
        id,
        author: Some(Name::new(env.caller.login.clone())),
        body,
        created_at: now,
        updated_at: now,
    };
    found
        .repo_mut(state)
        .issues
        .get_mut(&number)
        .ok_or_else(|| not_found("issue does not exist".to_owned()))?
        .comments
        .push(record.clone());
    let comment = crate::render::issue_comment(
        crate::handlers::issues::context(state, &request, &found),
        number,
        &record,
    );
    let mut ctx = events::Ctx::new(state, &request, &found.owner, &found.name);
    events::issue_comment::created(&mut ctx, number, &comment).map_err(webhook_err)?;
    Output::of(AddCommentPayload {
        typename: "AddCommentPayload",
        comment_edge: CommentEdge {
            node: CommentNode {
                typename: "IssueComment",
                id: comment.node_id,
                url: comment.html_url,
            },
        },
    })
}

fn add_pull_comment(
    state: &mut crate::state::State,
    subject_id: &str,
    body: String,
) -> Result<Output, Error> {
    for account in state.accounts.values_mut() {
        for repo in account.repos.values_mut() {
            for (number, pull) in &mut repo.pull_requests {
                let expected = node_id("PullRequest", pull_id(repo.id, *number));
                if subject_id == expected
                    || pull
                        .data
                        .as_ref()
                        .is_some_and(|data| data.node_id == subject_id)
                {
                    let Some(data) = pull.data.as_mut() else {
                        return Err(not_found(
                            "pull request REST model does not exist".to_owned(),
                        ));
                    };
                    let created_at = crate::time::now_utc();
                    let comment_number =
                        i64::try_from(pull.issue_comments.len() + 1).unwrap_or(i64::MAX);
                    let comment_id = node_id(
                        "IssueComment",
                        pull_id(repo.id, *number) + pull.issue_comments.len() as u64 + 1,
                    );
                    let html_url = format!("{}#issuecomment-{comment_number}", data.html_url);
                    pull.issue_comments.push(crate::types::IssueComment {
                        author_association: None,
                        body: Some(body.clone()),
                        body_html: None,
                        body_text: None,
                        created_at,
                        html_url: html_url.clone(),
                        id: comment_number,
                        issue_url: data.issue_url.clone(),
                        minimized: None,
                        node_id: comment_id.clone(),
                        performed_via_github_app: None,
                        pin: None,
                        reactions: None,
                        updated_at: created_at,
                        url: format!("{}/comments/{comment_number}", data.issue_url),
                        user: None,
                    });
                    data.comments = pull.issue_comments.len() as i64;
                    return Output::of(AddCommentPayload {
                        typename: "AddCommentPayload",
                        comment_edge: CommentEdge {
                            node: CommentNode {
                                typename: "IssueComment",
                                id: comment_id,
                                url: html_url,
                            },
                        },
                    });
                }
            }
        }
    }
    Err(not_found(format!(
        "Could not resolve comment subject '{subject_id}'"
    )))
}
