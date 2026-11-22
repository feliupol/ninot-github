//! GitHub REST error statuses and messages.
//!
//! Each [`GithubError`] variant owns its status. A caller-specific message is a field; every
//! other variant carries GitHub's message. [`ApiResponse`] constructors for success statuses
//! live on [`crate::api::ApiResponse`] and take no numeric code.

use std::borrow::Cow;

use http::StatusCode;

use crate::api::{ApiResponse, DOCS_URL};

mod message;

/// One GitHub REST error.
#[derive(Clone, Debug)]
pub(crate) enum GithubError {
    /// Source: <https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api#problems-parsing-json>
    ProblemsParsingJson,
    /// The same message with the parser's detail appended.
    ProblemsParsingJsonDetail {
        detail: String,
    },
    CannotReadBody,

    RequiresAuthentication,
    /// Source: <https://docs.github.com/en/rest/authentication/authenticating-to-the-rest-api#failed-login-limit>
    BadCredentials,
    GraphqlUnauthenticated,

    /// Source: <https://docs.github.com/en/apps/oauth-apps/building-oauth-apps/scopes-for-oauth-apps#available-scopes>
    PatForbidden,
    /// Source: <https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api#resource-not-accessible>
    IntegrationForbidden,
    AdminRequired,
    PushAccess,
    PushRepository,
    OrgOwnerOrMaintainer,
    CannotDeleteDefaultRunnerGroup,
    JobCannotBeRerun,
    WriteOrAuthor,
    OrgMemberCreateTeam,
    OrgMemberSeeMemberships,
    OrgOwnerRequired,
    RepoAdminOrTeam,
    RepoAdmin,
    OrgAdminBeforeAdding,
    /// Source: GitHub's 403 body for rulesets of a private repository on Free.
    PlanLimit,
    /// Source: <https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api#exceeding-the-rate-limit>
    RateLimitExceeded {
        who: String,
    },
    /// Source: <https://docs.github.com/en/rest/using-the-rest-api/rate-limits-for-the-rest-api#exceeding-the-rate-limit>
    SecondaryRateLimit,

    /// Source: <https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api#404-not-found-for-an-existing-resource>
    NotFound,
    BranchNotFound,
    BranchMissing {
        branch: String,
    },
    NoCommitForRef {
        reference: String,
    },
    RepositoryEmpty,
    BaseCommitNotFound,
    HeadCommitNotFound,

    MergeMethodNotAllowed,
    PullNotMergeable,
    /// 405 whose message comes from the caller.
    NotAllowed(Cow<'static, str>),

    RunnerNameExists,
    WorkflowRunAlreadyCompleted,
    GitRepositoryEmpty,
    ReferenceAlreadyExists,
    VariableAlreadyExists,
    VisibilityMustBeSelected,
    WorkflowRunNoFailedJobs,
    WorkflowRunNotCompleted,
    HeadShaMismatch,
    ShaMismatch {
        path: String,
        sha: String,
    },

    ArtifactExpired,
    BodyTooLarge,

    InvalidRef,
    InvalidBody,
    AuthorCannotApprove,
    BaseTreeInvalid,
    NotFastForward,
    BaseBranchMissing,
    ReviewBodyRequired,
    ContentNotBase64,
    HeadBranchMissing,
    ParentNotObject,
    TreeNotObject,
    OrgUrlRequired,
    CannotDeleteNonPendingReview,
    CannotDismissCommentedReview,
    CannotDismissDismissedReview,
    CannotDismissPendingReview,
    CannotSubmitNonPendingReview,
    ShaAndContent,
    CheckNotRerequestable,
    ShaNotSupplied,
    NoCommitForTheRef,
    EmptyRepository,
    PermissionsExceedGrant,
    RepositoriesExceedGrant,
    CannotRemoveDefaultGroupRunners,
    GitObjectMissing,
    NoNewCommits,
    WorkflowDispatchMissing,
    WorkflowDisabled,
    BaseHeadSeparator,
    BodyRequired,
    CouldNotAddRunner,
    HeadShaNotCurrent,
    FilesRequired,
    HeadNotInForkNetwork,
    KeyRequired,
    LabelsRequired,
    MergeConflict,
    MergeConflictBaseHead,
    NameRequired,
    ObjectMissing,
    ObjectTypeMismatch,
    HeadMustBeInRepository,
    PullNotOpen,
    ReviewersRequired,
    RunnerEventRequired,
    TitleRequired,
    TitleHeadBaseRequired,
    /// Source: GitHub's 422 body for a required reviewers rule on a private repository.
    ReviewersPlanLimit,
    UnsupportedEncoding {
        encoding: String,
    },
    NoCommitForSha {
        reference: String,
    },
    PathMissing {
        path: String,
    },
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

    NotImplemented {
        operation: String,
    },
    SortLongRunning,

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

    /// The HTTP status this error answers with.
    ///
    /// [`Self::Injected`] uses the rule's raw code; this returns a stand-in when that code is
    /// not a real status.
    #[allow(clippy::too_many_lines)]
    pub(crate) fn status(&self) -> StatusCode {
        use GithubError::*;
        match self {
            ProblemsParsingJson | ProblemsParsingJsonDetail { .. } | CannotReadBody => {
                StatusCode::BAD_REQUEST
            }
            RequiresAuthentication | BadCredentials | GraphqlUnauthenticated => {
                StatusCode::UNAUTHORIZED
            }
            PatForbidden
            | IntegrationForbidden
            | AdminRequired
            | PushAccess
            | PushRepository
            | OrgOwnerOrMaintainer
            | CannotDeleteDefaultRunnerGroup
            | JobCannotBeRerun
            | WriteOrAuthor
            | OrgMemberCreateTeam
            | OrgMemberSeeMemberships
            | OrgOwnerRequired
            | RepoAdminOrTeam
            | RepoAdmin
            | OrgAdminBeforeAdding
            | PlanLimit
            | RateLimitExceeded { .. }
            | SecondaryRateLimit => StatusCode::FORBIDDEN,
            NotFound
            | BranchNotFound
            | BranchMissing { .. }
            | NoCommitForRef { .. }
            | RepositoryEmpty
            | BaseCommitNotFound
            | HeadCommitNotFound => StatusCode::NOT_FOUND,
            MergeMethodNotAllowed | PullNotMergeable | NotAllowed(_) => {
                StatusCode::METHOD_NOT_ALLOWED
            }
            RunnerNameExists
            | WorkflowRunAlreadyCompleted
            | GitRepositoryEmpty
            | ReferenceAlreadyExists
            | VariableAlreadyExists
            | VisibilityMustBeSelected
            | WorkflowRunNoFailedJobs
            | WorkflowRunNotCompleted
            | HeadShaMismatch
            | ShaMismatch { .. } => StatusCode::CONFLICT,
            ArtifactExpired => StatusCode::GONE,
            BodyTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            InvalidRef
            | InvalidBody
            | AuthorCannotApprove
            | BaseTreeInvalid
            | NotFastForward
            | BaseBranchMissing
            | ReviewBodyRequired
            | ContentNotBase64
            | HeadBranchMissing
            | ParentNotObject
            | TreeNotObject
            | OrgUrlRequired
            | CannotDeleteNonPendingReview
            | CannotDismissCommentedReview
            | CannotDismissDismissedReview
            | CannotDismissPendingReview
            | CannotSubmitNonPendingReview
            | ShaAndContent
            | CheckNotRerequestable
            | ShaNotSupplied
            | NoCommitForTheRef
            | EmptyRepository
            | PermissionsExceedGrant
            | RepositoriesExceedGrant
            | CannotRemoveDefaultGroupRunners
            | GitObjectMissing
            | NoNewCommits
            | WorkflowDispatchMissing
            | WorkflowDisabled
            | BaseHeadSeparator
            | BodyRequired
            | CouldNotAddRunner
            | HeadShaNotCurrent
            | FilesRequired
            | HeadNotInForkNetwork
            | KeyRequired
            | LabelsRequired
            | MergeConflict
            | MergeConflictBaseHead
            | NameRequired
            | ObjectMissing
            | ObjectTypeMismatch
            | HeadMustBeInRepository
            | PullNotOpen
            | ReviewersRequired
            | RunnerEventRequired
            | TitleRequired
            | TitleHeadBaseRequired
            | ReviewersPlanLimit
            | UnsupportedEncoding { .. }
            | NoCommitForSha { .. }
            | PathMissing { .. }
            | Unprocessable(_)
            | ValidationFailed { .. } => StatusCode::UNPROCESSABLE_ENTITY,
            Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
            NotImplemented { .. } | SortLongRunning => StatusCode::NOT_IMPLEMENTED,
            Documented { status, .. } => *status,
            Injected { status, .. } => {
                StatusCode::from_u16(*status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR)
            }
        }
    }

    /// GitHub's message for this error.
    pub(crate) fn message(&self) -> Cow<'_, str> {
        message::message(self)
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
        message: Option<String>,
        status: Option<String>,
        url: Option<String>,
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

    fn render(error: GithubError) -> (u16, String, Option<String>) {
        let response = ApiResponse::from(error);
        let Body::Json(body) = response.body else {
            panic!("json body");
        };
        let parsed: Basic = serde_json::from_str(&body.to_string()).expect("basic error");
        assert_eq!(parsed.url, None);
        assert_eq!(parsed.documentation_url.as_deref(), Some(DOCS_URL));
        (
            response.status,
            parsed.message.expect("message"),
            parsed.status,
        )
    }

    fn expect(error: GithubError, status: StatusCode, message: &str) {
        let promised = (error.response_status(), error.message().into_owned());
        assert_eq!(promised.0, status.as_u16());
        assert_eq!(promised.1, message);
        let (got_status, got_message, body_status) = render(error);
        assert_eq!(got_status, status.as_u16());
        assert_eq!(got_message, message);
        assert_eq!(body_status.as_deref(), Some(status.as_str()));
    }

    #[test]
    fn every_variant_renders_its_status_and_message() {
        for (error, status, message) in [
            (
                GithubError::ProblemsParsingJson,
                StatusCode::BAD_REQUEST,
                "Problems parsing JSON",
            ),
            (
                GithubError::RequiresAuthentication,
                StatusCode::UNAUTHORIZED,
                "Requires authentication",
            ),
            (
                GithubError::AdminRequired,
                StatusCode::FORBIDDEN,
                "Must have admin rights to Repository.",
            ),
            (GithubError::NotFound, StatusCode::NOT_FOUND, "Not Found"),
            (
                GithubError::MergeConflict,
                StatusCode::UNPROCESSABLE_ENTITY,
                "merge conflict",
            ),
            (
                GithubError::NotImplemented {
                    operation: "repos/get".into(),
                },
                StatusCode::NOT_IMPLEMENTED,
                "Not implemented by ninot-github: repos/get",
            ),
        ] {
            expect(error, status, message);
        }
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
