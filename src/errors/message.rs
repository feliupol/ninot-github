//! GitHub's message for each [`super::GithubError`] variant.

use std::borrow::Cow;

use super::GithubError;

#[allow(clippy::too_many_lines)]
pub(super) fn message(error: &GithubError) -> Cow<'_, str> {
    use GithubError::*;
    match error {
        ProblemsParsingJson => Cow::Borrowed("Problems parsing JSON"),
        ProblemsParsingJsonDetail { detail } => {
            Cow::Owned(format!("Problems parsing JSON: {detail}"))
        }
        CannotReadBody => Cow::Borrowed("cannot read request body"),
        RequiresAuthentication => Cow::Borrowed("Requires authentication"),
        BadCredentials => Cow::Borrowed("Bad credentials"),
        GraphqlUnauthenticated => Cow::Borrowed("This endpoint requires you to be authenticated."),
        PatForbidden => Cow::Borrowed("Resource not accessible by personal access token"),
        IntegrationForbidden => Cow::Borrowed("Resource not accessible by integration"),
        AdminRequired => Cow::Borrowed("Must have admin rights to Repository."),
        PushAccess => Cow::Borrowed("Must have push access to this repository"),
        PushRepository => Cow::Borrowed("Must have push access to repository"),
        OrgOwnerOrMaintainer => {
            Cow::Borrowed("You must be an organization owner or team maintainer to do this.")
        }
        CannotDeleteDefaultRunnerGroup => Cow::Borrowed("Cannot delete the default runner group."),
        JobCannotBeRerun => Cow::Borrowed("Job cannot be rerun"),
        WriteOrAuthor => Cow::Borrowed("Must have write access or be the pull request author"),
        OrgMemberCreateTeam => {
            Cow::Borrowed("You must be an organization member to create a team.")
        }
        OrgMemberSeeMemberships => {
            Cow::Borrowed("You must be an organization member to see its memberships.")
        }
        OrgOwnerRequired => Cow::Borrowed("You must be an organization owner to do this."),
        RepoAdminOrTeam => {
            Cow::Borrowed("You must have admin access to the repository or maintain the team.")
        }
        RepoAdmin => Cow::Borrowed("You must have admin access to the repository."),
        OrgAdminBeforeAdding => Cow::Borrowed(
            "You need admin access to the organization before adding a repository to it.",
        ),
        PlanLimit => Cow::Borrowed(
            "Upgrade to GitHub Pro or make this repository public to enable this feature.",
        ),
        RateLimitExceeded { who } => Cow::Owned(format!("API rate limit exceeded for {who}.")),
        SecondaryRateLimit => Cow::Borrowed(
            "You have exceeded a secondary rate limit. Please wait a few minutes before you try again. If you reach out to GitHub Support for help, please include the request ID.",
        ),
        NotFound => Cow::Borrowed("Not Found"),
        BranchNotFound => Cow::Borrowed("Branch not found"),
        BranchMissing { branch } => Cow::Owned(format!("Branch {branch} not found")),
        NoCommitForRef { reference } => {
            Cow::Owned(format!("No commit found for the ref {reference}"))
        }
        RepositoryEmpty => Cow::Borrowed("This repository is empty."),
        BaseCommitNotFound => Cow::Borrowed("base commit not found"),
        HeadCommitNotFound => Cow::Borrowed("head commit not found"),
        MergeMethodNotAllowed => Cow::Borrowed("merge method is not allowed"),
        PullNotMergeable => Cow::Borrowed("pull request is not mergeable"),
        NotAllowed(message) | Unprocessable(message) | Internal(message) => message.clone(),
        RunnerNameExists => Cow::Borrowed("A runner with this name already exists."),
        WorkflowRunAlreadyCompleted => {
            Cow::Borrowed("Cannot cancel a workflow run that is completed")
        }
        GitRepositoryEmpty => Cow::Borrowed("Git Repository is empty."),
        ReferenceAlreadyExists => Cow::Borrowed("Reference already exists"),
        VariableAlreadyExists => Cow::Borrowed("Variable already exists"),
        VisibilityMustBeSelected => Cow::Borrowed("Visibility must be selected"),
        WorkflowRunNoFailedJobs => Cow::Borrowed("Workflow run has no failed jobs"),
        WorkflowRunNotCompleted => Cow::Borrowed("Workflow run is not completed"),
        HeadShaMismatch => Cow::Borrowed("head SHA does not match"),
        ShaMismatch { path, sha } => Cow::Owned(format!("{path} does not match {sha}")),
        ArtifactExpired => Cow::Borrowed("Artifact has expired"),
        BodyTooLarge => Cow::Borrowed("Request body too large"),
        InvalidRef => Cow::Borrowed("invalid ref"),
        InvalidBody => Cow::Borrowed("Invalid request body"),
        AuthorCannotApprove => Cow::Borrowed(
            "A pull request author cannot approve or request changes on their own pull request",
        ),
        BaseTreeInvalid => Cow::Borrowed("The base_tree is not a valid git tree"),
        NotFastForward => Cow::Borrowed("Update is not a fast forward"),
        BaseBranchMissing => Cow::Borrowed("base branch does not exist"),
        ReviewBodyRequired => Cow::Borrowed("body is required for this review event"),
        ContentNotBase64 => Cow::Borrowed("content is not valid Base64"),
        HeadBranchMissing => Cow::Borrowed("head branch does not exist"),
        ParentNotObject => Cow::Borrowed("parent is not a valid git object"),
        TreeNotObject => Cow::Borrowed("tree is not a valid git object"),
        OrgUrlRequired => Cow::Borrowed("url must name an organization"),
        CannotDeleteNonPendingReview => {
            Cow::Borrowed("Can not delete a non-pending pull request review")
        }
        CannotDismissCommentedReview => {
            Cow::Borrowed("Can not dismiss a commented pull request review")
        }
        CannotDismissDismissedReview => {
            Cow::Borrowed("Can not dismiss a dismissed pull request review")
        }
        CannotDismissPendingReview => {
            Cow::Borrowed("Can not dismiss a pending pull request review")
        }
        CannotSubmitNonPendingReview => {
            Cow::Borrowed("Can not submit a non-pending pull request review")
        }
        ShaAndContent => Cow::Borrowed("Cannot specify both sha and content for a tree entry"),
        CheckNotRerequestable => Cow::Borrowed("Check run is not rerequestable"),
        ShaNotSupplied => Cow::Borrowed("Invalid request.\n\n\"sha\" wasn't supplied."),
        NoCommitForTheRef => Cow::Borrowed("No commit found for the ref"),
        EmptyRepository => Cow::Borrowed("Repository is empty."),
        PermissionsExceedGrant => {
            Cow::Borrowed("Requested permissions exceed the installation grant")
        }
        RepositoriesExceedGrant => {
            Cow::Borrowed("Requested repositories exceed the installation grant")
        }
        CannotRemoveDefaultGroupRunners => {
            Cow::Borrowed("Runners cannot be removed from the default runner group.")
        }
        GitObjectMissing => Cow::Borrowed("The git object does not exist."),
        NoNewCommits => Cow::Borrowed("There are no new commits on the base branch."),
        WorkflowDispatchMissing => {
            Cow::Borrowed("Workflow does not have 'workflow_dispatch' trigger")
        }
        WorkflowDisabled => Cow::Borrowed("Workflow is disabled"),
        BaseHeadSeparator => Cow::Borrowed("base and head must be separated by ..."),
        BodyRequired => Cow::Borrowed("body is required"),
        CouldNotAddRunner => Cow::Borrowed("could not add the runner"),
        HeadShaNotCurrent => Cow::Borrowed("expected head sha didn't match current head ref."),
        FilesRequired => Cow::Borrowed("files is required"),
        HeadNotInForkNetwork => Cow::Borrowed("head branch does not exist in this fork network"),
        KeyRequired => Cow::Borrowed("key is required"),
        LabelsRequired => Cow::Borrowed("labels must contain at least one label"),
        MergeConflict => Cow::Borrowed("merge conflict"),
        MergeConflictBaseHead => Cow::Borrowed("merge conflict between base and head"),
        NameRequired => Cow::Borrowed("name is required"),
        ObjectMissing => Cow::Borrowed("object does not exist"),
        ObjectTypeMismatch => Cow::Borrowed("object type does not match the provided type"),
        HeadMustBeInRepository => Cow::Borrowed(
            "only pull requests whose head branch is in this repository can be updated",
        ),
        PullNotOpen => Cow::Borrowed("pull request is not open"),
        ReviewersRequired => Cow::Borrowed("reviewers or team_reviewers is required"),
        RunnerEventRequired => Cow::Borrowed("runner_event must be register or remove"),
        TitleRequired => Cow::Borrowed("title is required"),
        TitleHeadBaseRequired => Cow::Borrowed("title, head and base are required"),
        ReviewersPlanLimit => Cow::Borrowed(
            "Failed to create the environment protection rule. Please ensure the billing plan supports the required reviewers protection rule.",
        ),
        UnsupportedEncoding { encoding } => {
            Cow::Owned(format!("The encoding '{encoding}' is not supported"))
        }
        NoCommitForSha { reference } => Cow::Owned(format!("No commit found for SHA: {reference}")),
        PathMissing { path } => Cow::Owned(format!("The path {path} does not exist in the tree")),
        ValidationFailed { .. } => Cow::Borrowed("Validation Failed"),
        NotImplemented { operation } => {
            Cow::Owned(format!("Not implemented by ninot-github: {operation}"))
        }
        SortLongRunning => Cow::Borrowed("sort=long-running is not implemented by ninot-github"),
        Documented { message, .. } | Injected { message, .. } => message.clone(),
    }
}
