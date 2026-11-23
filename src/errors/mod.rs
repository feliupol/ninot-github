//! GitHub REST error statuses and messages.
//!
//! Each [`GithubError`] variant owns its status. A caller-specific message is a field; every
//! other variant carries GitHub's message. [`ApiResponse`] constructors for success statuses
//! live on [`crate::api::ApiResponse`] and take no numeric code.
//!
//! The `github_errors!` invocation below defines every variant once: a row under its status
//! block, `Variant => "message"` or `Variant { field: String } => "message with {field}"`.
//! A variant whose status or message is computed goes in the `custom` section instead.
//!
//! To add a variant, add its row, then add its `Name\tstatus\tmessage` line to `golden.tsv`
//! in sorted order. A `custom` variant also needs a value in the test's `all_variants`.

use std::borrow::Cow;

use http::StatusCode;

use crate::api::{ApiResponse, DOCS_URL};

#[macro_use]
mod table;

github_errors! {
    custom {
        variants {
            /// 405 whose message comes from the caller.
            NotAllowed(Cow<'static, str>),
            /// A 422 whose message comes from the caller.
            Unprocessable(Cow<'static, str>),
            /// Source: <https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api#validation-failed>
            ValidationFailed {
                resource: Cow<'static, str>,
                field: Cow<'static, str>,
                code: Cow<'static, str>,
            },
            /// A 500 whose message comes from the caller.
            Internal(Cow<'static, str>),
            /// Basic error whose documentation link is the operation's, not [`DOCS_URL`].
            Documented {
                status: StatusCode,
                documentation_url: Cow<'static, str>,
                message: Cow<'static, str>,
            },
            /// A fault rule's status. The raw code is kept even when it is not a standard status.
            Injected {
                status: u16,
                message: Cow<'static, str>,
            },
        }
        status {
            Self::NotAllowed(_) => StatusCode::METHOD_NOT_ALLOWED,
            Self::Unprocessable(_) | Self::ValidationFailed { .. } => {
                StatusCode::UNPROCESSABLE_ENTITY
            }
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Documented { status, .. } => *status,
            Self::Injected { status, .. } => {
                StatusCode::from_u16(*status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
        message {
            Self::NotAllowed(message) | Self::Unprocessable(message) | Self::Internal(message) => {
                message.clone()
            }
            Self::ValidationFailed { .. } => Cow::Borrowed("Validation Failed"),
            Self::Documented { message, .. } | Self::Injected { message, .. } => message.clone(),
        }
    }

    BAD_REQUEST {
        /// Source: <https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api#problems-parsing-json>
        ProblemsParsingJson => "Problems parsing JSON",
        /// The same message with the parser's detail appended.
        ProblemsParsingJsonDetail { detail: String } => "Problems parsing JSON: {detail}",
        CannotReadBody => "cannot read request body",
    }

    UNAUTHORIZED {
        RequiresAuthentication => "Requires authentication",
        /// Source: <https://docs.github.com/en/rest/authentication/authenticating-to-the-rest-api#failed-login-limit>
        BadCredentials => "Bad credentials",
        GraphqlUnauthenticated => "This endpoint requires you to be authenticated.",
    }

    FORBIDDEN {
        /// Source: <https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/scopes-for-oauth-apps#available-scopes>
        PatForbidden => "Resource not accessible by personal access token",
        /// Source: <https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api#resource-not-accessible>
        IntegrationForbidden => "Resource not accessible by integration",
        AdminRequired => "Must have admin rights to Repository.",
        PushAccess => "Must have push access to this repository",
        PushRepository => "Must have push access to repository",
        OrgOwnerOrMaintainer => "You must be an organization owner or team maintainer to do this.",
        CannotDeleteDefaultRunnerGroup => "Cannot delete the default runner group.",
        JobCannotBeRerun => "Job cannot be rerun",
        WriteOrAuthor => "Must have write access or be the pull request author",
        OrgMemberCreateTeam => "You must be an organization member to create a team.",
        OrgMemberSeeMemberships => "You must be an organization member to see its memberships.",
        OrgOwnerRequired => "You must be an organization owner to do this.",
        RepoAdminOrTeam => "You must have admin access to the repository or maintain the team.",
        RepoAdmin => "You must have admin access to the repository.",
        OrgAdminBeforeAdding =>
            "You need admin access to the organization before adding a repository to it.",
        /// Source: GitHub's 403 body for rulesets of a private repository on Free.
        PlanLimit => "Upgrade to GitHub Pro or make this repository public to enable this feature.",
        /// Source: <https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api#exceeding-the-rate-limit>
        RateLimitExceeded { who: String } => "API rate limit exceeded for {who}.",
        /// Source: <https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api#exceeding-the-rate-limit>
        SecondaryRateLimit => "You have exceeded a secondary rate limit. Please wait a few minutes before you try again. If you reach out to GitHub Support for help, please include the request ID.",
    }

    NOT_FOUND {
        /// Source: <https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api#404-not-found-for-an-existing-resource>
        NotFound => "Not Found",
        BranchNotFound => "Branch not found",
        BranchMissing { branch: String } => "Branch {branch} not found",
        NoCommitForRef { reference: String } => "No commit found for the ref {reference}",
        RepositoryEmpty => "This repository is empty.",
        BaseCommitNotFound => "base commit not found",
        HeadCommitNotFound => "head commit not found",
    }

    METHOD_NOT_ALLOWED {
        MergeMethodNotAllowed => "merge method is not allowed",
        PullNotMergeable => "pull request is not mergeable",
    }

    CONFLICT {
        RunnerNameExists => "A runner with this name already exists.",
        WorkflowRunAlreadyCompleted => "Cannot cancel a workflow run that is completed",
        GitRepositoryEmpty => "Git Repository is empty.",
        ReferenceAlreadyExists => "Reference already exists",
        VariableAlreadyExists => "Variable already exists",
        VisibilityMustBeSelected => "Visibility must be selected",
        WorkflowRunNoFailedJobs => "Workflow run has no failed jobs",
        WorkflowRunNotCompleted => "Workflow run is not completed",
        HeadShaMismatch => "head SHA does not match",
        ShaMismatch { path: String, sha: String } => "{path} does not match {sha}",
    }

    GONE {
        ArtifactExpired => "Artifact has expired",
    }

    PAYLOAD_TOO_LARGE {
        BodyTooLarge => "Request body too large",
    }

    UNPROCESSABLE_ENTITY {
        InvalidRef => "invalid ref",
        InvalidBody => "Invalid request body",
        AuthorCannotApprove =>
            "A pull request author cannot approve or request changes on their own pull request",
        BaseTreeInvalid => "The base_tree is not a valid git tree",
        NotFastForward => "Update is not a fast forward",
        BaseBranchMissing => "base branch does not exist",
        ReviewBodyRequired => "body is required for this review event",
        ContentNotBase64 => "content is not valid Base64",
        HeadBranchMissing => "head branch does not exist",
        ParentNotObject => "parent is not a valid git object",
        TreeNotObject => "tree is not a valid git object",
        OrgUrlRequired => "url must name an organization",
        CannotDeleteNonPendingReview => "Can not delete a non-pending pull request review",
        CannotDismissCommentedReview => "Can not dismiss a commented pull request review",
        CannotDismissDismissedReview => "Can not dismiss a dismissed pull request review",
        CannotDismissPendingReview => "Can not dismiss a pending pull request review",
        CannotSubmitNonPendingReview => "Can not submit a non-pending pull request review",
        ShaAndContent => "Cannot specify both sha and content for a tree entry",
        CheckNotRerequestable => "Check run is not rerequestable",
        ShaNotSupplied => "Invalid request.\n\n\"sha\" wasn't supplied.",
        NoCommitForTheRef => "No commit found for the ref",
        EmptyRepository => "Repository is empty.",
        PermissionsExceedGrant => "Requested permissions exceed the installation grant",
        RepositoriesExceedGrant => "Requested repositories exceed the installation grant",
        CannotRemoveDefaultGroupRunners =>
            "Runners cannot be removed from the default runner group.",
        GitObjectMissing => "The git object does not exist.",
        NoNewCommits => "There are no new commits on the base branch.",
        WorkflowDispatchMissing => "Workflow does not have 'workflow_dispatch' trigger",
        WorkflowDisabled => "Workflow is disabled",
        BaseHeadSeparator => "base and head must be separated by ...",
        BodyRequired => "body is required",
        CouldNotAddRunner => "could not add the runner",
        HeadShaNotCurrent => "expected head sha didn't match current head ref.",
        FilesRequired => "files is required",
        HeadNotInForkNetwork => "head branch does not exist in this fork network",
        KeyRequired => "key is required",
        LabelsRequired => "labels must contain at least one label",
        MergeConflict => "merge conflict",
        MergeConflictBaseHead => "merge conflict between base and head",
        NameRequired => "name is required",
        ObjectMissing => "object does not exist",
        ObjectTypeMismatch => "object type does not match the provided type",
        HeadMustBeInRepository =>
            "only pull requests whose head branch is in this repository can be updated",
        PullNotOpen => "pull request is not open",
        ReviewersRequired => "reviewers or team_reviewers is required",
        RunnerEventRequired => "runner_event must be register or remove",
        TitleRequired => "title is required",
        TitleHeadBaseRequired => "title, head and base are required",
        /// Source: GitHub's 422 body for a required reviewers rule on a private repository.
        ReviewersPlanLimit => "Failed to create the environment protection rule. Please ensure the billing plan supports the required reviewers protection rule.",
        UnsupportedEncoding { encoding: String } => "The encoding '{encoding}' is not supported",
        NoCommitForSha { reference: String } => "No commit found for SHA: {reference}",
        PathMissing { path: String } => "The path {path} does not exist in the tree",
    }

    NOT_IMPLEMENTED {
        NotImplemented { operation: String } => "Not implemented by ninot-github: {operation}",
        SortLongRunning => "sort=long-running is not implemented by ninot-github",
    }
}

impl GithubError {
    /// An injected fault. Statuses of 500 and above say "Server Error".
    pub(crate) fn injected(status: u16) -> Self {
        let message = if status >= 500 {
            "Server Error"
        } else {
            "Injected fault"
        };
        Self::Injected {
            status,
            message: Cow::Borrowed(message),
        }
    }

    fn response_status(&self) -> u16 {
        match self {
            Self::Injected { status, .. } => *status,
            other => other.status().as_u16(),
        }
    }
}

impl GithubError {
    /// The REST response for this error. Named so callers need no type annotation.
    pub(crate) fn to_response(&self) -> ApiResponse {
        let status = self.response_status();
        match self {
            GithubError::ValidationFailed {
                resource,
                field,
                code,
            } => ApiResponse::validation_body(resource, field, code),
            GithubError::Documented {
                documentation_url,
                message,
                ..
            } => ApiResponse::basic_error_raw(status, documentation_url, message),
            _ => ApiResponse::basic_error_raw(status, DOCS_URL, self.message().as_ref()),
        }
    }
}

impl From<GithubError> for ApiResponse {
    fn from(error: GithubError) -> Self {
        error.to_response()
    }
}

/// Whether `status` is an HTTP success code.
pub(crate) fn is_success(status: u16) -> bool {
    StatusCode::from_u16(status).is_ok_and(|status| status.is_success())
}

/// Whether `status` is 200.
pub(crate) fn is_ok(status: u16) -> bool {
    status == StatusCode::OK.as_u16()
}

/// Whether `status` is 404.
pub(crate) fn is_not_found(status: u16) -> bool {
    status == StatusCode::NOT_FOUND.as_u16()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::Body;

    #[derive(serde::Deserialize)]
    struct Basic {
        documentation_url: Option<String>,
    }

    #[derive(serde::Deserialize)]
    struct Validation {
        documentation_url: String,
        message: String,
        status: String,
        errors: Vec<ValidationItem>,
    }

    #[derive(serde::Deserialize)]
    struct ValidationItem {
        code: String,
        field: Option<String>,
        resource: Option<String>,
    }

    /// Every table row from [`GithubError::samples`], then each `custom` variant.
    fn all_variants() -> Vec<GithubError> {
        let mut variants = GithubError::samples();
        variants.extend([
            GithubError::NotAllowed("NotAllowed".into()),
            GithubError::Unprocessable("Unprocessable".into()),
            GithubError::ValidationFailed {
                resource: "resource".into(),
                field: "field".into(),
                code: "code".into(),
            },
            GithubError::Internal("Internal".into()),
            GithubError::Documented {
                status: StatusCode::BAD_REQUEST,
                documentation_url: "documentation_url".into(),
                message: "message".into(),
            },
            GithubError::injected(418),
            GithubError::injected(503),
        ]);
        variants
    }

    /// `Name\tstatus\tmessage` for the HTTP response of `error`, with `\\`, newline and tab escaped.
    fn golden_line(error: GithubError) -> String {
        let debug = format!("{error:?}");
        let name: String = debug
            .chars()
            .take_while(char::is_ascii_alphanumeric)
            .collect();
        let response = ApiResponse::from(error);
        let Body::Json(body) = response.body else {
            panic!("json body");
        };
        let message = body["message"]
            .as_str()
            .expect("message")
            .replace('\\', "\\\\")
            .replace('\n', "\\n")
            .replace('\t', "\\t");
        format!("{name}\t{}\t{message}", response.status)
    }

    /// `golden.tsv` lists every variant's response, sorted. Add a line for a variant; never
    /// change an existing one without changing the behaviour on purpose.
    #[test]
    fn every_variant_matches_golden() {
        let mut lines: Vec<String> = all_variants().into_iter().map(golden_line).collect();
        lines.sort();
        let actual = lines.join("\n") + "\n";
        assert_eq!(actual, include_str!("golden.tsv"));
    }

    #[test]
    fn validation_failed_and_documented_keep_their_body_shapes() {
        let failed = GithubError::ValidationFailed {
            resource: "Repository".into(),
            field: "name".into(),
            code: "custom".into(),
        };
        assert_eq!(failed.message(), "Validation Failed");
        let response = ApiResponse::from(failed);
        assert_eq!(response.status, StatusCode::UNPROCESSABLE_ENTITY.as_u16());
        let Body::Json(body) = response.body else {
            panic!("json")
        };
        let parsed: Validation = serde_json::from_str(&body.to_string()).unwrap();
        assert_eq!(parsed.message, "Validation Failed");
        assert_eq!(parsed.status, StatusCode::UNPROCESSABLE_ENTITY.as_str());
        assert_eq!(parsed.documentation_url, DOCS_URL);
        assert_eq!(parsed.errors[0].resource.as_deref(), Some("Repository"));
        assert_eq!(parsed.errors[0].field.as_deref(), Some("name"));
        assert_eq!(parsed.errors[0].code, "custom");

        let documented = GithubError::Documented {
            status: StatusCode::BAD_REQUEST,
            documentation_url: "https://docs.github.com/rest/repos/repos".into(),
            message: "Problems parsing JSON".into(),
        };
        let response = ApiResponse::from(documented);
        let Body::Json(body) = response.body else {
            panic!("json")
        };
        let parsed: Basic = serde_json::from_str(&body.to_string()).unwrap();
        assert_eq!(
            parsed.documentation_url.as_deref(),
            Some("https://docs.github.com/rest/repos/repos")
        );
    }
}
