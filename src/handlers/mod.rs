//! Operation handlers and their registry.

use std::sync::Mutex;

use crate::api::{ApiRequest, ApiResult};
use crate::operation::{Operation, OperationInfo, ops};
use crate::state::State;
use ninot_common::git;

/// Author and committer of the commits the fake makes itself, such as a new repository's
/// README and the tags of a release.
pub(crate) const BOT: git::Bot = git::Bot {
    name: "ninot-github",
    email: "ninot-github@localhost",
};

mod actions;
mod actions_secrets;
pub(crate) mod actions_service;
mod apps;
mod check_suites;
mod checks;
mod checks_model;
mod gists;
mod git_blobs;
mod git_commits;
mod git_data;
mod git_store;
mod git_tags;
mod git_trees;
mod hooks;
mod issues;
mod meta;
mod pulls;
pub mod releases;
pub mod repos;
pub(crate) mod runner_service;
mod runners;
pub(crate) mod search;
pub(crate) mod timeline;
mod users;

/// Implemented by the generated type of every operation the fake supports.
pub trait Handler: Operation {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult;
}

/// Function pointer to a handler.
pub type HandlerFn = fn(&Mutex<State>, &ApiRequest) -> ApiResult;

macro_rules! registry {
    ($($op:ty),* $(,)?) => {
        /// Operations that have a handler.
        pub static IMPLEMENTED: &[&OperationInfo] = &[$(<$op as Operation>::INFO),*];

        /// The handler of an operation, if it has one.
        pub fn handler(info: &OperationInfo) -> Option<HandlerFn> {
            $(if info == <$op as Operation>::INFO {
                return Some(<$op as Handler>::handle);
            })*
            None
        }
    };
}

registry![
    ops::GistsList,
    ops::GistsCreate,
    ops::AppsGetRepoInstallation,
    ops::AppsCreateInstallationAccessToken,
    ops::MetaRoot,
    ops::MetaGetZen,
    ops::RateLimitGet,
    ops::OrgsGet,
    ops::UsersGetAuthenticated,
    ops::UsersGetByUsername,
    ops::ReposCreateForAuthenticatedUser,
    ops::ReposCreateInOrg,
    ops::ReposGet,
    ops::ReposListForAuthenticatedUser,
    ops::ReposListForOrg,
    ops::ReposListForUser,
    ops::ReposUpdate,
    ops::ReposDelete,
    ops::ReposTransfer,
    ops::GitGetRef,
    ops::GitCreateRef,
    ops::GitListMatchingRefs,
    ops::GitCreateBlob,
    ops::GitGetBlob,
    ops::GitCreateCommit,
    ops::GitGetCommit,
    ops::GitUpdateRef,
    ops::GitCreateTag,
    ops::GitGetTag,
    ops::GitCreateTree,
    ops::GitGetTree,
    ops::ReposGetCommit,
    ops::ReposListBranches,
    ops::ActionsListWorkflowRunsForRepo,
    ops::ActionsGetWorkflowRun,
    ops::ActionsReRunWorkflow,
    ops::ActionsReRunWorkflowFailedJobs,
    ops::ActionsGetRepoPublicKey,
    ops::ActionsListRepoSecrets,
    ops::ActionsGetRepoSecret,
    ops::ActionsCreateOrUpdateRepoSecret,
    ops::ActionsDeleteRepoSecret,
    ops::ActionsListJobsForWorkflowRun,
    ops::ActionsGetJobForWorkflowRun,
    ops::ActionsDownloadJobLogsForWorkflowRun,
    ops::ActionsDownloadWorkflowRunLogs,
    ops::ChecksListForRef,
    ops::ChecksGet,
    ops::ChecksCreate,
    ops::ChecksUpdate,
    ops::ChecksListAnnotations,
    ops::ChecksRerequestRun,
    ops::ChecksCreateSuite,
    ops::ChecksSetSuitesPreferences,
    ops::ChecksGetSuite,
    ops::ChecksListForSuite,
    ops::ChecksRerequestSuite,
    ops::ChecksListSuitesForRef,
    ops::ReposListCommitStatusesForRef,
    ops::ReposGetCombinedStatusForRef,
    ops::ReposListPullRequestsAssociatedWithCommit,
    ops::ReposGetBranch,
    ops::ReposGetContent,
    ops::ReposCreateOrUpdateFileContents,
    ops::ReposCompareCommits,
    ops::ReposCreateRelease,
    ops::ReposListReleases,
    ops::ReposGetLatestRelease,
    ops::ReposGetReleaseByTag,
    ops::ReposGetRelease,
    ops::ReposUpdateRelease,
    ops::ReposDeleteRelease,
    ops::ReposListReleaseAssets,
    ops::PullsGet,
    ops::PullsUpdate,
    ops::PullsList,
    ops::PullsCreate,
    ops::PullsCreateReview,
    ops::PullsListReviews,
    ops::PullsListRequestedReviewers,
    ops::PullsRequestReviewers,
    ops::PullsRemoveRequestedReviewers,
    ops::PullsListCommits,
    ops::PullsListFiles,
    ops::IssuesListEventsForTimeline,
    ops::IssuesListEvents,
    ops::IssuesListEventsForRepo,
    ops::IssuesGetEvent,
    ops::IssuesListComments,
    ops::PullsListReviewComments,
    ops::IssuesAddLabels,
    ops::IssuesCreateComment,
    ops::IssuesCreate,
    ops::IssuesListForRepo,
    ops::IssuesGet,
    ops::IssuesListLabelsForRepo,
    ops::IssuesListLabelsOnIssue,
    ops::IssuesUpdate,
    ops::IssuesAddAssignees,
    ops::IssuesRemoveAssignees,
    ops::IssuesListAssignees,
    ops::IssuesCheckUserCanBeAssigned,
    ops::IssuesCheckUserCanBeAssignedToIssue,
    ops::IssuesSetLabels,
    ops::IssuesRemoveAllLabels,
    ops::IssuesRemoveLabel,
    ops::IssuesCreateLabel,
    ops::IssuesGetLabel,
    ops::IssuesUpdateLabel,
    ops::IssuesDeleteLabel,
    ops::IssuesListMilestones,
    ops::IssuesCreateMilestone,
    ops::IssuesGetMilestone,
    ops::IssuesUpdateMilestone,
    ops::IssuesDeleteMilestone,
    ops::IssuesListLabelsForMilestone,
    ops::IssuesGetComment,
    ops::IssuesUpdateComment,
    ops::IssuesDeleteComment,
    ops::IssuesListCommentsForRepo,
    ops::IssuesLock,
    ops::IssuesUnlock,
    ops::IssuesList,
    ops::IssuesListForOrg,
    ops::IssuesListForAuthenticatedUser,
    ops::SearchIssuesAndPullRequests,
    ops::SearchRepos,
    ops::PullsMerge,
    ops::GitDeleteRef,
    ops::ReposUploadReleaseAsset,
    ops::ReposGetReleaseAsset,
    ops::ReposDeleteReleaseAsset,
    ops::ReposCreateWebhook,
    ops::ReposListWebhooks,
    ops::ReposGetWebhook,
    ops::ReposUpdateWebhook,
    ops::ReposDeleteWebhook,
    ops::ReposPingWebhook,
    ops::ReposListWebhookDeliveries,
    ops::ActionsCreateRegistrationTokenForOrg,
    ops::ActionsCreateRemoveTokenForOrg,
    ops::ActionsGenerateRunnerJitconfigForOrg,
    ops::ActionsListSelfHostedRunnersForOrg,
    ops::ActionsGetSelfHostedRunnerForOrg,
    ops::ActionsDeleteSelfHostedRunnerFromOrg,
    ops::ActionsListLabelsForSelfHostedRunnerForOrg,
    ops::ActionsAddCustomLabelsToSelfHostedRunnerForOrg,
    ops::ActionsSetCustomLabelsForSelfHostedRunnerForOrg,
    ops::ActionsRemoveAllCustomLabelsFromSelfHostedRunnerForOrg,
    ops::ActionsRemoveCustomLabelFromSelfHostedRunnerForOrg,
    ops::ActionsListSelfHostedRunnerGroupsForOrg,
    ops::ActionsCreateSelfHostedRunnerGroupForOrg,
    ops::ActionsGetSelfHostedRunnerGroupForOrg,
    ops::ActionsUpdateSelfHostedRunnerGroupForOrg,
    ops::ActionsDeleteSelfHostedRunnerGroupFromOrg,
    ops::ActionsSetSelfHostedRunnersInGroupForOrg,
];
