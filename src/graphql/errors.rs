//! GraphQL error messages.
//!
//! Plain constructors build an [`Error`] with no `type` extension. [`not_found`] and
//! [`typed_error`] set GitHub's `type` the way the resolvers did before.

use std::fmt::Display;

use async_graphql::Error;
use async_graphql::ErrorExtensions;

/// An error with no GraphQL `type` extension.
pub(super) fn plain(message: impl Into<String>) -> Error {
    Error::new(message)
}

/// GitHub's error for a missing object, with its `NOT_FOUND` type.
pub(super) fn not_found(message: impl Into<String>) -> Error {
    typed_error(message, "NOT_FOUND")
}

/// An error that GitHub reports with a top-level `type`.
pub(super) fn typed_error(message: impl Into<String>, kind: &'static str) -> Error {
    Error::new(message).extend_with(|_, extensions| extensions.set("type", kind))
}

/// The GraphQL `type` a REST status becomes when a mutation reuses a REST error body.
pub(super) fn rest_kind(status: u16) -> &'static str {
    if status == http::StatusCode::FORBIDDEN.as_u16() {
        "FORBIDDEN"
    } else if status == http::StatusCode::NOT_FOUND.as_u16() {
        "NOT_FOUND"
    } else {
        "UNPROCESSABLE"
    }
}

pub(super) fn cannot_serialize(error: &impl Display) -> Error {
    plain(format!("cannot serialize the result: {error}"))
}

pub(super) fn unexpected_args(error: &impl Display) -> Error {
    plain(format!("unexpected arguments or parent: {error}"))
}

pub(super) fn not_implemented(key: &str) -> Error {
    plain(format!("Not implemented by ninot-github: {key}"))
}

pub(super) fn unknown_state(what: &str, text: &str) -> Error {
    plain(format!("{what} has the unknown state {text:?}"))
}

pub(super) fn could_not_read_commit(sha: &str, error: &(impl Display + ?Sized)) -> Error {
    plain(format!("could not read commit {sha}: {error}"))
}

pub(super) fn repository_missing() -> Error {
    plain("repository does not exist")
}

pub(super) fn pull_missing() -> Error {
    plain("pull request does not exist")
}

pub(super) fn pull_rest_missing() -> Error {
    plain("pull request REST model does not exist")
}

pub(super) fn viewer_missing() -> Error {
    plain("viewer does not exist")
}

pub(super) fn missing() -> Error {
    plain("missing")
}

pub(super) fn review_not_stored() -> Error {
    plain("review was not stored")
}

pub(super) fn no_git_storage() -> Error {
    plain("repository has no git storage")
}

pub(super) fn resolve_pull_number(number: impl Display) -> Error {
    plain(format!(
        "Could not resolve to a PullRequest with the number of {number}."
    ))
}

pub(super) fn repository_not_found() -> Error {
    not_found("repository does not exist")
}

pub(super) fn pull_not_found() -> Error {
    not_found("pull request does not exist")
}

pub(super) fn pull_rest_not_found() -> Error {
    not_found("pull request REST model does not exist")
}

pub(super) fn issue_not_found() -> Error {
    not_found("issue does not exist")
}

pub(super) fn viewer_not_found() -> Error {
    not_found("viewer does not exist")
}

pub(super) fn ref_without_repository() -> Error {
    not_found("ref has no repository")
}

pub(super) fn ref_missing() -> Error {
    not_found("ref does not exist")
}

pub(super) fn head_ref_missing() -> Error {
    not_found("head ref does not exist")
}

pub(super) fn destination_repository_missing() -> Error {
    not_found("destination repository does not exist")
}

pub(super) fn resolve_repository() -> Error {
    not_found("Could not resolve to a Repository.")
}

pub(super) fn resolve_labelable() -> Error {
    not_found("Could not resolve labelable.")
}

pub(super) fn resolve_destination() -> Error {
    not_found("Could not resolve destination repository.")
}

pub(super) fn resolve_repository_short() -> Error {
    not_found("Could not resolve repository.")
}

pub(super) fn resolve_project_field_value() -> Error {
    not_found("Could not resolve project item field value")
}

pub(super) fn resolve_node(id: impl Display) -> Error {
    not_found(format!(
        "Could not resolve to a node with the global id of '{id}'"
    ))
}

pub(super) fn resolve_user(login: impl Display) -> Error {
    not_found(format!("Could not resolve to a User '{login}'"))
}

pub(super) fn resolve_user_login(login: &str) -> Error {
    not_found(format!(
        "Could not resolve to a User with the login of '{login}'."
    ))
}

pub(super) fn resolve_org_login(login: &str) -> Error {
    not_found(format!(
        "Could not resolve to an Organization with the login of '{login}'."
    ))
}

pub(super) fn resolve_repository_name(owner: &str, name: &str) -> Error {
    not_found(format!(
        "Could not resolve to a Repository with the name '{owner}/{name}'."
    ))
}

pub(super) fn resolve_issue_number(number: impl Display) -> Error {
    not_found(format!(
        "Could not resolve to an issue with the number of {number}."
    ))
}

pub(super) fn resolve_issue_or_pull(number: impl Display) -> Error {
    not_found(format!(
        "Could not resolve to an Issue or PullRequest with the number of {number}."
    ))
}

pub(super) fn resolve_review_thread(id: impl Display) -> Error {
    not_found(format!("Could not resolve review thread '{id}'"))
}

pub(super) fn resolve_comment_subject(id: impl Display) -> Error {
    not_found(format!("Could not resolve comment subject '{id}'"))
}

pub(super) fn resolve_project(id: impl Display) -> Error {
    not_found(format!("Could not resolve project '{id}'"))
}

pub(super) fn resolve_project_item(id: impl Display) -> Error {
    not_found(format!("Could not resolve project item '{id}'"))
}

pub(super) fn resolve_project_option(id: impl Display) -> Error {
    not_found(format!("Could not resolve project option '{id}'"))
}

pub(super) fn integration_forbidden() -> Error {
    typed_error("Resource not accessible by integration", "FORBIDDEN")
}

pub(super) fn pat_forbidden() -> Error {
    typed_error(
        "Resource not accessible by personal access token",
        "FORBIDDEN",
    )
}

pub(super) fn admin_required() -> Error {
    typed_error("Must have admin rights to Repository.", "FORBIDDEN")
}

pub(super) fn push_access() -> Error {
    typed_error("Must have push access to this repository", "FORBIDDEN")
}

pub(super) fn write_or_author() -> Error {
    typed_error(
        "Must have write access or be the pull request author",
        "FORBIDDEN",
    )
}

pub(super) fn transfer_same_owner() -> Error {
    typed_error(
        "Issues can only be transferred to a repository owned by the same user or organization.",
        "FORBIDDEN",
    )
}

pub(super) fn create_repository_permission(login: &str) -> Error {
    typed_error(
        format!("{login} does not have the correct permissions to execute `CreateRepository`"),
        "FORBIDDEN",
    )
}

pub(super) fn insufficient_scopes() -> Error {
    typed_error(
        "Your token has not been granted the required scopes to execute this query.",
        "INSUFFICIENT_SCOPES",
    )
}

pub(super) fn no_git_storage_unprocessable() -> Error {
    typed_error("repository has no git storage", "UNPROCESSABLE")
}

pub(super) fn pull_not_open() -> Error {
    typed_error("pull request is not open", "UNPROCESSABLE")
}

pub(super) fn head_sha_not_current() -> Error {
    typed_error(
        "expected head sha didn't match current head ref.",
        "UNPROCESSABLE",
    )
}

pub(super) fn no_new_commits() -> Error {
    typed_error(
        "There are no new commits on the base branch.",
        "UNPROCESSABLE",
    )
}

pub(super) fn title_required() -> Error {
    typed_error("title is required", "UNPROCESSABLE")
}

pub(super) fn base_branch_missing() -> Error {
    typed_error("base branch does not exist", "UNPROCESSABLE")
}

pub(super) fn no_merge_commit() -> Error {
    typed_error("pull request has no merge commit", "UNPROCESSABLE")
}

pub(super) fn name_exists() -> Error {
    typed_error("Name already exists on this account", "UNPROCESSABLE")
}

pub(super) fn pinned_limit() -> Error {
    typed_error(
        "Repository has reached the maximum of 3 pinned issues.",
        "UNPROCESSABLE",
    )
}

pub(super) fn only_merged_can_revert() -> Error {
    typed_error(
        "only a merged pull request can be reverted",
        "UNPROCESSABLE",
    )
}

pub(super) fn auto_merge_not_allowed() -> Error {
    typed_error(
        "auto merge is not allowed for this repository",
        "UNPROCESSABLE",
    )
}

pub(super) fn dismiss_own_mutation() -> Error {
    typed_error("dismiss a review through its own mutation", "UNPROCESSABLE")
}

pub(super) fn review_body_required() -> Error {
    typed_error("body is required for this review event", "UNPROCESSABLE")
}

pub(super) fn author_cannot_approve() -> Error {
    typed_error(
        "A pull request author cannot approve or request changes on their own pull request",
        "UNPROCESSABLE",
    )
}

pub(super) fn branch_missing(branch: &str) -> Error {
    typed_error(format!("{branch} does not exist"), "UNPROCESSABLE")
}
