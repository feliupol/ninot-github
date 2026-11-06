"""Typed Python inputs and outputs for the Ninot GitHub fixture API."""

from typing import Literal, Optional, TypedDict, Union

ActionsSecretsPermission = Literal["read", "write"]


class ActionsPublicKeySeed(TypedDict):
    """Repository-scoped public key used by Actions secret clients."""

    key_id: str
    key: str


class ActionsSecretSeed(TypedDict):
    """An already-encrypted secret seeded into a repository fixture."""

    name: str
    encrypted_value: str
    key_id: str


class UserSeed(TypedDict, total=False):
    """Sparse user attributes accepted in fixture seeds."""

    id: int
    login: str
    node_id: str
    avatar_url: str
    email: Optional[str]
    events_url: str
    followers_url: str
    following_url: str
    gists_url: str
    gravatar_id: Optional[str]
    url: str
    html_url: str
    name: Optional[str]
    type: str
    organizations_url: str
    received_events_url: str
    repos_url: str
    site_admin: bool
    starred_at: Optional[str]
    starred_url: str
    subscriptions_url: str
    user_view_type: Optional[str]


class LabelSeed(TypedDict, total=False):
    """Sparse label attributes accepted in issue and pull-request seeds."""

    id: int
    node_id: str
    url: str
    name: str
    description: Optional[str]
    color: str
    default: bool


class PullRequestBranchSeed(TypedDict, total=False):
    """Sparse branch information nested in a pull-request seed."""

    label: str
    ref: str
    sha: str
    repo: "RepositoryReferenceSeed"
    user: Optional[UserSeed]


class RepositoryReferenceSeed(TypedDict, total=False):
    """Sparse repository identity nested in a pull-request branch."""

    full_name: str


class AutoMergeSeed(TypedDict, total=False):
    """Sparse auto-merge details nested in a pull-request seed."""

    commit_message: str
    commit_title: str
    enabled_by: UserSeed
    merge_method: Literal["merge", "squash", "rebase"]


class IssueSeed(TypedDict, total=False):
    """Sparse typed values for creating an issue fixture."""

    title: str
    body: Optional[str]
    state: Literal["open", "closed"]
    labels: list[LabelSeed]
    comments: int
    locked: bool
    assignee: Optional[UserSeed]
    assignees: list[UserSeed]


class TeamSeed(TypedDict):
    """A team of the repository owner's organization, created when it does not exist."""

    slug: str


class PullRequestSeed(TypedDict, total=False):
    """Sparse typed values for creating a pull-request fixture."""

    id: int
    number: int
    title: str
    html_url: str
    created_at: str
    updated_at: str
    user: UserSeed
    body: Optional[str]
    state: Literal["open", "closed"]
    draft: bool
    mergeable: Optional[bool]
    labels: list[LabelSeed]
    base: PullRequestBranchSeed
    head: PullRequestBranchSeed
    requested_reviewers: list[UserSeed]
    requested_teams: list[TeamSeed]
    auto_merge: Optional[AutoMergeSeed]
    merged: bool
    comments: int
    locked: bool
    maintainer_can_modify: bool
    commits: int
    changed_files: int
    additions: int
    deletions: int
    merged_at: str
    closed_at: str
    merged_by: UserSeed
    files: list["PullRequestFileSeed"]


class _PullRequestFileSeedRequired(TypedDict):
    filename: str


class PullRequestFileSeed(_PullRequestFileSeedRequired, total=False):
    """A changed file listed by the pull-request files endpoint."""

    status: Literal["added", "removed", "modified", "renamed", "copied", "changed", "unchanged"]
    additions: int
    deletions: int
    previous_filename: str


TimelineEventName = Literal[
    "ready_for_review",
    "convert_to_draft",
    "review_requested",
    "review_request_removed",
    "merged",
    "closed",
    "reopened",
]


class _TimelineEventSeedRequired(TypedDict):
    event: TimelineEventName
    created_at: str


class TimelineEventSeed(_TimelineEventSeedRequired, total=False):
    """A pull-request timeline event, applied as the action would be."""

    actor: UserSeed
    requested_reviewer: UserSeed
    requested_team: TeamSeed
    commit_id: str


class GitIdentitySeed(TypedDict):
    """The author or committer of a fixture commit."""

    name: str
    email: str
    date: str


class ReviewSeed(TypedDict, total=False):
    """Sparse typed values for a pull-request review."""

    id: int
    state: Literal["APPROVED", "CHANGES_REQUESTED", "COMMENTED", "DISMISSED", "PENDING"]
    body: str
    user: Optional[UserSeed]
    commit_id: Optional[str]
    submitted_at: Optional[str]


class IssueCommentSeed(TypedDict, total=False):
    """Sparse typed values for an issue comment."""

    body: str
    user: Optional[UserSeed]
    created_at: str
    updated_at: str


class ReviewCommentSeed(TypedDict, total=False):
    """Sparse typed values for a pull-request review comment."""

    path: str
    line: Optional[int]
    position: Optional[int]
    body: str
    user: Optional[UserSeed]
    created_at: str
    updated_at: str
    pull_request_review_id: int
    in_reply_to_id: int


class ReviewThreadCommentSeed(TypedDict, total=False):
    """Sparse comment values in a GraphQL review thread."""

    body: str
    updatedAt: str


class ReviewThreadSeed(TypedDict, total=False):
    """Sparse persisted GraphQL review-thread values."""

    id: str
    isResolved: bool
    path: Optional[str]
    line: Optional[int]
    comments: list[ReviewThreadCommentSeed]


class WorkflowRunSeed(TypedDict, total=False):
    """Sparse typed values for an Actions workflow run."""

    id: int
    name: Optional[str]
    head_branch: Optional[str]
    head_sha: str
    html_url: str
    event: str
    status: Optional[str]
    conclusion: Optional[str]
    display_title: str
    path: str
    workflow_id: int
    run_number: int
    created_at: str
    updated_at: str


class JobStepSeed(TypedDict, total=False):
    """Sparse typed values for an Actions job step."""

    number: int
    name: str
    status: str
    conclusion: str


class JobSeed(TypedDict, total=False):
    """Sparse typed values for an Actions job."""

    id: int
    name: str
    status: str
    conclusion: Optional[str]
    started_at: Optional[str]
    completed_at: Optional[str]
    steps: list[JobStepSeed]


class CheckRunOutputSeed(TypedDict, total=False):
    """Sparse output values for an Actions check run."""

    title: Optional[str]
    summary: Optional[str]
    text: Optional[str]
    annotations_count: int
    annotations_url: str


class CheckRunCheckSuiteSeed(TypedDict, total=False):
    """Sparse check-suite identity embedded in a check run."""

    id: int


class IntegrationPermissionsSeed(TypedDict, total=False):
    """Sparse permissions assigned to a GitHub App integration."""

    checks: str
    contents: str
    deployments: str
    issues: str
    metadata: str


class IntegrationSeed(TypedDict, total=False):
    """Sparse GitHub App metadata embedded in a check run."""

    id: int
    node_id: str
    slug: Optional[str]
    name: str
    description: Optional[str]
    external_url: str
    html_url: str
    created_at: str
    updated_at: str
    owner: UserSeed
    permissions: IntegrationPermissionsSeed
    events: list[str]


class PullRequestMinimalRepositorySeed(TypedDict):
    """Repository identity in a check-run pull-request association."""

    id: int
    name: str
    url: str


class PullRequestMinimalBranchSeed(TypedDict):
    """Branch identity in a check-run pull-request association."""

    ref: str
    repo: PullRequestMinimalRepositorySeed
    sha: str


class PullRequestMinimalSeed(TypedDict):
    """Pull-request association included with a check run."""

    base: PullRequestMinimalBranchSeed
    head: PullRequestMinimalBranchSeed
    id: int
    number: int
    url: str


class CheckRunSeed(TypedDict, total=False):
    """Sparse typed values for a check run attached to a commit."""

    id: int
    node_id: str
    name: str
    head_sha: str
    url: str
    html_url: Optional[str]
    status: Literal["queued", "in_progress", "completed"]
    conclusion: Optional[str]
    details_url: Optional[str]
    external_id: Optional[str]
    check_suite: Optional[CheckRunCheckSuiteSeed]
    app: Optional[IntegrationSeed]
    pull_requests: list[PullRequestMinimalSeed]
    started_at: Optional[str]
    completed_at: Optional[str]
    output: CheckRunOutputSeed


class CommitStatusSeed(TypedDict, total=False):
    """Sparse typed values for a commit status."""

    id: int
    state: Literal["error", "failure", "pending", "success"]
    context: str
    description: Optional[str]
    target_url: Optional[str]
    creator: Optional[UserSeed]
    created_at: str
    updated_at: str


class OperationFaultTarget(TypedDict):
    """A REST operation target used by fault injection."""

    operation: str


class GitFaultTarget(TypedDict):
    """A Git transport target used by fault injection."""

    git: str


class PathPrefixFaultTarget(TypedDict):
    """A request-path prefix target used by fault injection."""

    path_prefix: str


FaultTarget = Union[Literal["any"], OperationFaultTarget, GitFaultTarget, PathPrefixFaultTarget]


class LatencyFault(TypedDict):
    """A request latency fault."""

    latency_ms: int


class StatusFault(TypedDict):
    """An HTTP status fault."""

    status: int


FaultValue = Union[
    LatencyFault,
    StatusFault,
    Literal["secondary_rate_limit", "drop", "truncate"],
]


class _FaultRuleRequired(TypedDict):
    target: FaultTarget
    fault: FaultValue


class FaultRule(_FaultRuleRequired, total=False):
    """Typed fault-injection rule accepted by Server.add_fault."""

    times: int
    probability: float


class ProfileHit(TypedDict):
    """One observed HTTP request."""

    method: str
    path: str
    query: Optional[str]
    request: str
    label: str
    status: int
    request_bytes: int
    response_bytes: int
    duration_ms: int


class ProfileLabel(TypedDict):
    """Aggregated request timing for one route label."""

    label: str
    count: int
    total_ms: int


class ProfileRepeat(TypedDict):
    """Count of one repeated request."""

    request: str
    count: int


class Profile(TypedDict):
    """Request profile returned by Server.stop_profile."""

    requests: int
    hits: list[ProfileHit]
    labels: list[ProfileLabel]
    repeats: list[ProfileRepeat]


class AppliedFault(TypedDict):
    """One applied fault and the request it affected."""

    method: str
    path: str
    target: str
    fault: FaultValue
