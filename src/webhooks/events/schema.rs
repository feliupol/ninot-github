//! Builds every webhook payload from the fixture models and checks the pinned schema.
//!
//! The HTTP suite records the same deliveries. This test does not open a socket, so a payload
//! that does not match its schema fails here.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use super::pull_request;
use super::workflow_run;
use super::{Ctx, PayloadError, check_run, check_suite, create, delete, issue_comment, issues};
use super::{pull_request_review, pull_request_review_comment, release, status, workflow_job};
use crate::api::ApiRequest;
use crate::fixtures::{self, IssueSeed, LabelSeed, PullRequestSeed, SeedField};
use crate::render;
use crate::state::{Hook, Permission, State};
use crate::types;

#[test]
fn every_event_payload_matches_its_schema() {
    let mut state = State::default();
    state.validate_payloads = true;
    state.add_user("admin");
    state.create_repo("admin", "tool", false, None).unwrap();
    state
        .account_mut("admin")
        .unwrap()
        .repo_mut("tool")
        .unwrap()
        .hooks
        .push(hook());
    let request = ApiRequest::for_urls("http://127.0.0.1:9", None);
    let user = {
        let account = state.account("admin").unwrap();
        render::simple_user(&request, account)
    };
    let repository = {
        let account = state.account("admin").unwrap();
        let repo = account.repo("tool").unwrap();
        render::repository_for_pull(
            &request,
            account,
            repo,
            Permission {
                admin: true,
                push: true,
                pull: true,
            },
        )
    };

    let label = LabelSeed {
        name: SeedField::Present("bug".to_owned()),
        ..LabelSeed::default()
    };
    let issue = fixtures::seeded_issue(
        &mut state,
        &request,
        "admin",
        "tool",
        user.clone(),
        1,
        IssueSeed {
            title: SeedField::Present("Bug".to_owned()),
            labels: SeedField::Present(vec![label.clone()]),
            assignee: SeedField::Present(Some(fixtures::UserSeed {
                login: SeedField::Present("admin".to_owned()),
                ..fixtures::UserSeed::default()
            })),
            ..IssueSeed::default()
        },
    )
    .unwrap();
    state.add_issue("admin", "tool", 1, issue.clone());
    let pull = fixtures::seeded_pull_request(
        &mut state,
        &request,
        ("admin", "tool"),
        user.clone(),
        repository,
        2,
        PullRequestSeed {
            title: SeedField::Present("Draft".to_owned()),
            labels: SeedField::Present(vec![label]),
            ..PullRequestSeed::default()
        },
    )
    .unwrap()
    .data;

    let sha = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    let mut before = BTreeMap::new();
    let mut after = BTreeMap::new();
    before.insert(
        "refs/heads/other".to_owned(),
        super::push::ZERO_SHA.to_owned(),
    );
    after.insert("refs/heads/other".to_owned(), sha.to_owned());
    let mut failures = Vec::new();
    {
        let mut ctx = Ctx::new(&mut state, &request, "admin", "tool");
        issues::opened(&mut ctx, &issue).unwrap();
        for item in issues::issue_labels(&issue) {
            issues::labeled(&mut ctx, &issue, &item).unwrap();
            issues::unlabeled(&mut ctx, &issue, &item).unwrap();
        }
        issues::edited(&mut ctx, &issue, Some("Earlier"), Some("")).unwrap();
        let mut closed = issue.clone();
        closed.state = "closed".to_owned();
        closed.closed_at = Some(crate::time::now_utc());
        issues::closed(&mut ctx, &closed).unwrap();
        issues::reopened(&mut ctx, &issue).unwrap();
        issues::assigned(&mut ctx, &issue, &user).unwrap();
        issues::unassigned(&mut ctx, &issue, &user).unwrap();
        let mut locked = issue.clone();
        locked.locked = true;
        issues::locked(&mut ctx, &locked).unwrap();
        issues::unlocked(&mut ctx, &issue).unwrap();

        pull_request::opened(&mut ctx, &pull).unwrap();
        let mut changes = Map::new();
        changes.insert("title".to_owned(), json!({ "from": "Draft" }));
        changes.insert("body".to_owned(), json!({ "from": "" }));
        changes.insert(
            "base".to_owned(),
            json!({ "ref": { "from": "main" }, "sha": { "from": pull.base.sha } }),
        );
        pull_request::edited(&mut ctx, &pull, changes).unwrap();
        let mut closed_pull = pull.clone();
        closed_pull.state = types::PullRequestState::Closed;
        closed_pull.closed_at = Some(crate::time::now_utc());
        pull_request::closed(&mut ctx, &closed_pull).unwrap();
        pull_request::reopened(&mut ctx, &pull).unwrap();
        pull_request::synchronize(&mut ctx, &pull, &pull.base.sha, &pull.head.sha, &user).unwrap();
        for item in pull_request::pull_labels(&pull) {
            pull_request::labeled(&mut ctx, &pull, &item).unwrap();
        }
        pull_request::review_requested_user(&mut ctx, &pull, &user).unwrap();
        let team = fixtures::requested_team(ctx.state, &request, "acme", "reviewers").unwrap();
        pull_request::review_requested_team(&mut ctx, &pull, &team).unwrap();

        let review = review(&request, &user, &pull);
        pull_request_review::submitted(&mut ctx, &pull, &review).unwrap();
        let comment = review_comment(&request, &user, pull.number);
        pull_request_review_comment::created(&mut ctx, &pull, &comment).unwrap();
        let issue_note = issue_comment(&request, &user, 1);
        issue_comment::created(&mut ctx, 1, &issue_note).unwrap();
        issue_comment::edited(&mut ctx, 1, &issue_note, "old").unwrap();
        issue_comment::deleted(&mut ctx, 1, &issue_note).unwrap();
        ctx.state.add_pull_request("admin", "tool", 2, pull.clone());
        issue_comment::created(&mut ctx, 2, &issue_note).unwrap();

        let check = fixtures::seeded_check_run(
            ctx.state,
            &request,
            sha,
            fixtures::CheckRunSeed::default(),
            0,
        )
        .unwrap();
        let check = render_run(ctx.state, &request, &check);
        check_suite::requested(&mut ctx, &check).unwrap();
        check_run::created(&mut ctx, &check).unwrap();
        check_run::completed(&mut ctx, &check).unwrap();
        // A rerequest follows a finished run, so its suite carries a conclusion.
        let finished = fixtures::seeded_check_run(
            ctx.state,
            &request,
            sha,
            fixtures::CheckRunSeed {
                status: fixtures::SeedField::Present("completed".to_owned()),
                conclusion: fixtures::SeedField::Present(Some("success".to_owned())),
                ..fixtures::CheckRunSeed::default()
            },
            1,
        )
        .unwrap();
        let finished = render_run(ctx.state, &request, &finished);
        check_run::rerequested(&mut ctx, &finished).unwrap();

        let (run, job) = {
            let account = ctx.state.account("admin").unwrap();
            let repo = account.repo("tool").unwrap();
            let run = fixtures::seeded_workflow_run(fixtures::WorkflowRunSeed::default()).unwrap();
            let job = fixtures::seeded_job(fixtures::JobSeed::default(), 0).unwrap();
            (
                crate::render::workflow_run(&request, account, repo, &run),
                crate::render::workflow_job(&request, "admin", "tool", &run, &job),
            )
        };
        let mut queued_run = run.clone();
        queued_run.status = Some("queued".to_owned());
        queued_run.conclusion = None;
        let mut queued_job = job.clone();
        queued_job.status = types::JobStatus::Queued;
        queued_job.conclusion = None;
        workflow_run::deliver(&mut ctx, &run);
        workflow_job::deliver(&mut ctx, &run, &job);
        workflow_run::deliver(&mut ctx, &queued_run);
        workflow_job::deliver(&mut ctx, &queued_run, &queued_job);
        let mut running_run = run.clone();
        running_run.status = Some("in_progress".to_owned());
        running_run.conclusion = None;
        let mut running_job = job.clone();
        running_job.status = types::JobStatus::InProgress;
        running_job.conclusion = None;
        workflow_run::deliver(&mut ctx, &running_run);
        workflow_job::deliver(&mut ctx, &running_run, &running_job);
        let mut waiting_job = queued_job.clone();
        waiting_job.status = types::JobStatus::Waiting;
        workflow_job::deliver(&mut ctx, &queued_run, &waiting_job);

        let status_model = {
            let account = ctx.state.account("admin").unwrap();
            let record = fixtures::seeded_commit_status(
                &request,
                account,
                fixtures::CommitStatusSeed::default(),
                0,
            )
            .unwrap();
            crate::handlers::status_model::status(
                &request.urls().repo("admin", "tool"),
                sha,
                &record,
            )
        };
        status::commit_status(&mut ctx, sha, &status_model).unwrap();

        let published = sample_release(user);
        release::deliver(&mut ctx, "created", &published, None).unwrap();
        release::deliver(&mut ctx, "published", &published, None).unwrap();
        let mut prerelease = published.clone();
        prerelease.prerelease = true;
        release::deliver(&mut ctx, "prereleased", &prerelease, None).unwrap();
        release::deliver(&mut ctx, "unpublished", &published, None).unwrap();
        release::deliver(&mut ctx, "deleted", &published, None).unwrap();
        let mut edited = Map::new();
        edited.insert("name".to_owned(), json!({ "from": "Draft" }));
        edited.insert("body".to_owned(), json!({ "from": "" }));
        release::deliver(&mut ctx, "edited", &published, Some(edited)).unwrap();

        create::reference(&mut ctx, "topic", "branch");
        create::reference(&mut ctx, "v1", "tag");
        delete::reference(&mut ctx, "topic", "branch");
        delete::reference(&mut ctx, "v1", "tag");
    }
    let shared = std::sync::Mutex::new(state);
    create::from_push(&shared, &request, "admin", "tool", &before, &after);
    delete::from_push(&shared, &request, "admin", "tool", &after, &before);

    let state = shared.into_inner().unwrap();
    for delivery in &state.deliveries {
        if let Some(error) = &delivery.error {
            failures.push(format!(
                "{} {}\n{error}",
                delivery.event,
                delivery.payload().get("action").unwrap_or(&Value::Null)
            ));
        }
    }
    assert!(
        failures.is_empty(),
        "{} schema failures:\n{}",
        failures.len(),
        failures.join("\n---\n")
    );
    for (event, action) in [
        ("pull_request", "opened"),
        ("pull_request", "edited"),
        ("pull_request", "closed"),
        ("pull_request", "reopened"),
        ("pull_request", "synchronize"),
        ("pull_request", "labeled"),
        ("pull_request", "review_requested"),
        ("pull_request_review", "submitted"),
        ("pull_request_review_comment", "created"),
        ("issues", "opened"),
        ("issues", "labeled"),
        ("issues", "unlabeled"),
        ("issues", "edited"),
        ("issues", "closed"),
        ("issues", "reopened"),
        ("issues", "assigned"),
        ("issues", "unassigned"),
        ("issues", "locked"),
        ("issues", "unlocked"),
        ("issue_comment", "created"),
        ("issue_comment", "edited"),
        ("issue_comment", "deleted"),
        ("check_run", "created"),
        ("check_run", "completed"),
        ("check_run", "rerequested"),
        ("check_suite", "requested"),
        ("workflow_run", "completed"),
        ("workflow_run", "requested"),
        ("workflow_job", "completed"),
        ("workflow_job", "queued"),
        ("workflow_job", "in_progress"),
        ("workflow_job", "waiting"),
        ("workflow_run", "in_progress"),
        ("release", "created"),
        ("release", "published"),
        ("release", "prereleased"),
        ("release", "edited"),
        ("release", "unpublished"),
        ("release", "deleted"),
        ("create", "branch"),
        ("delete", "tag"),
        ("status", ""),
    ] {
        assert!(
            state.deliveries.iter().any(|delivery| {
                delivery.event == event
                    && delivery.error.is_none()
                    && (action.is_empty()
                        || delivery.payload().get("action").and_then(Value::as_str) == Some(action)
                        || delivery.payload().get("ref_type").and_then(Value::as_str)
                            == Some(action))
            }),
            "missing clean delivery {event} {action}"
        );
    }
}

fn hook() -> Hook {
    let now = crate::state::now();
    Hook {
        id: 1,
        active: true,
        events: vec!["*".to_owned()],
        url: "http://127.0.0.1:9/hook".to_owned(),
        content_type: "json".to_owned(),
        secret: None,
        insecure_ssl: "0".to_owned(),
        created_at: now.clone(),
        updated_at: now,
    }
}

fn sample_release(author: types::SimpleUser) -> types::Release {
    let now = crate::time::now_utc();
    types::Release {
        assets: Vec::new(),
        assets_url: "https://api.github.com/repos/admin/tool/releases/1/assets".to_owned(),
        author,
        body: Some("notes".to_owned()),
        body_html: None,
        body_text: None,
        created_at: now,
        discussion_url: None,
        draft: false,
        html_url: "https://github.com/admin/tool/releases/tag/v1".to_owned(),
        id: 1,
        immutable: Some(false),
        mentions_count: None,
        name: Some("v1".to_owned()),
        node_id: "R_1".to_owned(),
        prerelease: false,
        published_at: Some(now),
        reactions: None,
        tag_name: "v1".to_owned(),
        tarball_url: Some("https://api.github.com/repos/admin/tool/tarball/v1".to_owned()),
        target_commitish: "main".to_owned(),
        updated_at: Some(now),
        upload_url: "https://uploads.github.com/repos/admin/tool/releases/1/assets{?name,label}"
            .to_owned(),
        url: "https://api.github.com/repos/admin/tool/releases/1".to_owned(),
        zipball_url: Some("https://api.github.com/repos/admin/tool/zipball/v1".to_owned()),
    }
}

fn review(
    request: &ApiRequest,
    user: &types::SimpleUser,
    pull: &types::PullRequest,
) -> types::PullRequestReview {
    let now = crate::time::now_utc();
    types::PullRequestReview {
        author_association: types::AuthorAssociation::None,
        body: "Looks fine".to_owned(),
        body_html: None,
        body_text: None,
        commit_id: Some(pull.head.sha.clone()),
        html_url: format!("{}/pull/2#pullrequestreview-1", request.web_url),
        id: 1,
        links: types::PullRequestReviewLinks {
            html: types::PullRequestReviewLinksHtml {
                href: format!("{}/pull/2#pullrequestreview-1", request.web_url),
            },
            pull_request: types::PullRequestReviewLinksPullRequest {
                href: format!("{}/pulls/2", request.api_url),
            },
        },
        node_id: "PRR_1".to_owned(),
        pull_request_url: format!("{}/pulls/2", request.api_url),
        state: "COMMENTED".to_owned(),
        submitted_at: Some(now),
        user: Some(user.clone().into()),
    }
}

fn review_comment(
    request: &ApiRequest,
    user: &types::SimpleUser,
    number: i64,
) -> types::PullRequestReviewComment {
    let now = crate::time::now_utc();
    let api = request.api_url.as_str();
    let web = request.web_url.as_str();
    types::PullRequestReviewComment {
        author_association: types::AuthorAssociation::None,
        body: "Here".to_owned(),
        body_html: None,
        body_text: None,
        commit_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        created_at: now,
        diff_hunk: "@@ -1 +1 @@".to_owned(),
        html_url: format!("{web}/pull/{number}#discussion-diff-1"),
        id: 1,
        in_reply_to_id: None,
        line: Some(1),
        links: types::PullRequestReviewCommentLinks {
            html: types::PullRequestReviewCommentLinksHtml {
                href: format!("{web}/pull/{number}#discussion-diff-1"),
            },
            pull_request: types::PullRequestReviewCommentLinksPullRequest {
                href: format!("{api}/pulls/{number}"),
            },
            self_: types::PullRequestReviewCommentLinksSelf {
                href: format!("{api}/pulls/comments/1"),
            },
        },
        node_id: "PRRC_1".to_owned(),
        original_commit_id: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_owned(),
        original_line: Some(1),
        original_position: Some(1),
        original_start_line: None,
        path: "README.md".to_owned(),
        position: Some(1),
        pull_request_review_id: Some(1),
        pull_request_url: format!("{api}/pulls/{number}"),
        reactions: None,
        side: types::PullRequestReviewCommentSide::Right,
        start_line: None,
        start_side: None,
        subject_type: None,
        updated_at: now,
        url: format!("{api}/pulls/comments/1"),
        user: Some(user.clone().into()),
    }
}

fn issue_comment(
    request: &ApiRequest,
    user: &types::SimpleUser,
    number: i64,
) -> types::IssueComment {
    let now = crate::time::now_utc();
    types::IssueComment {
        author_association: Some(types::AuthorAssociation::None),
        body: Some("I see it".to_owned()),
        body_html: None,
        body_text: None,
        created_at: now,
        html_url: format!("{}/issues/{number}#issuecomment-1", request.web_url),
        id: 1,
        issue_url: format!("{}/issues/{number}", request.api_url),
        minimized: None,
        node_id: "IC_1".to_owned(),
        performed_via_github_app: None,
        pin: None,
        reactions: None,
        updated_at: now,
        url: format!("{}/issues/comments/1", request.api_url),
        user: Some(user.clone().into()),
    }
}

fn render_run(
    state: &crate::state::State,
    request: &ApiRequest,
    run: &crate::state::CheckRunRecord,
) -> types::CheckRun {
    let account = state.account("admin").unwrap();
    crate::handlers::checks_model::render_run(request, account, "tool", run)
}

/// A repository `admin/tool` with a hook for every event, and payload validation on.
fn hooked_state() -> (State, ApiRequest) {
    let mut state = State::default();
    state.validate_payloads = true;
    state.add_user("admin");
    state.create_repo("admin", "tool", false, None).unwrap();
    state
        .account_mut("admin")
        .unwrap()
        .repo_mut("tool")
        .unwrap()
        .hooks
        .push(hook());
    (state, ApiRequest::for_urls("http://127.0.0.1:9", None))
}

/// The only delivery of `state`, which must match its schema.
fn sole_payload(state: &State) -> Value {
    let [delivery] = &state.deliveries[..] else {
        panic!("{} deliveries", state.deliveries.len());
    };
    assert_eq!(delivery.error, None, "{}", delivery.payload());
    delivery.payload()
}

#[test]
fn a_check_without_an_app_names_the_stand_in_app_with_its_events() {
    let (mut state, request) = hooked_state();
    let check = fixtures::seeded_check_run(
        &mut state,
        &request,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        fixtures::CheckRunSeed::default(),
        0,
    )
    .unwrap();
    let check = render_run(&state, &request, &check);
    check_run::created(&mut Ctx::new(&mut state, &request, "admin", "tool"), &check).unwrap();
    let payload = sole_payload(&state);
    assert_eq!(payload["check_run"]["app"]["name"], "ninot-github");
    assert_eq!(payload["check_run"]["app"]["events"], json!([]));
}

#[test]
fn a_pending_check_run_is_created_queued() {
    let (mut state, request) = hooked_state();
    let check = fixtures::seeded_check_run(
        &mut state,
        &request,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        fixtures::CheckRunSeed {
            status: SeedField::Present("pending".to_owned()),
            ..fixtures::CheckRunSeed::default()
        },
        0,
    )
    .unwrap();
    let check = render_run(&state, &request, &check);
    check_run::created(&mut Ctx::new(&mut state, &request, "admin", "tool"), &check).unwrap();
    assert_eq!(sole_payload(&state)["check_run"]["status"], "queued");
}

#[test]
fn a_rerequested_skipped_run_has_a_neutral_suite() {
    let (mut state, request) = hooked_state();
    let check = fixtures::seeded_check_run(
        &mut state,
        &request,
        "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
        fixtures::CheckRunSeed {
            status: SeedField::Present("completed".to_owned()),
            conclusion: SeedField::Present(Some("skipped".to_owned())),
            ..fixtures::CheckRunSeed::default()
        },
        0,
    )
    .unwrap();
    let check = render_run(&state, &request, &check);
    check_run::rerequested(&mut Ctx::new(&mut state, &request, "admin", "tool"), &check).unwrap();
    let payload = sole_payload(&state);
    assert_eq!(payload["check_run"]["conclusion"], "skipped");
    assert_eq!(payload["check_run"]["check_suite"]["conclusion"], "neutral");
}

#[test]
fn an_open_release_asset_is_delivered_uploaded() {
    let (mut state, request) = hooked_state();
    let author = render::simple_user(&request, state.account("admin").unwrap());
    let mut published = sample_release(author);
    let now = crate::time::now_utc();
    published.assets.push(types::ReleaseAsset {
        browser_download_url: "https://github.com/admin/tool/releases/download/v1/a.zip".to_owned(),
        content_type: "application/zip".to_owned(),
        created_at: now,
        digest: None,
        download_count: 0,
        id: 1,
        label: None,
        name: "a.zip".to_owned(),
        node_id: "RA_1".to_owned(),
        size: 1,
        state: types::ReleaseAssetState::Open,
        updated_at: now,
        uploader: None,
        url: "https://api.github.com/repos/admin/tool/releases/assets/1".to_owned(),
    });
    release::deliver(
        &mut Ctx::new(&mut state, &request, "admin", "tool"),
        "published",
        &published,
        None,
    )
    .unwrap();
    assert_eq!(
        sole_payload(&state)["release"]["assets"][0]["state"],
        "uploaded"
    );
}

#[test]
fn an_issue_label_given_by_name_is_delivered_as_a_label() {
    let (mut state, request) = hooked_state();
    let user = render::simple_user(&request, state.account("admin").unwrap());
    let mut issue = fixtures::seeded_issue(
        &mut state,
        &request,
        "admin",
        "tool",
        user,
        1,
        IssueSeed::default(),
    )
    .unwrap();
    issue.labels = vec![types::IssueLabelsItem::String("bug".to_owned())];
    issues::opened(&mut Ctx::new(&mut state, &request, "admin", "tool"), &issue).unwrap();
    let label = &sole_payload(&state)["issue"]["labels"][0];
    assert_eq!(label["name"], "bug");
    assert_eq!(label["color"], "ededed");
}

/// An issue #1 in the repository of `hooked_state`.
fn seeded_issue(state: &mut State, request: &ApiRequest) -> types::Issue {
    let user = render::simple_user(request, state.account("admin").unwrap());
    fixtures::seeded_issue(
        state,
        request,
        "admin",
        "tool",
        user,
        1,
        IssueSeed::default(),
    )
    .unwrap()
}

/// The only delivery of `state` records `error` and carries no payload.
fn assert_unbuilt(state: &State, error: &PayloadError) {
    let [delivery] = &state.deliveries[..] else {
        panic!("{} deliveries", state.deliveries.len());
    };
    assert_eq!(
        delivery.error.as_deref(),
        Some(format!("ninot-github could not build the payload: {error}").as_str())
    );
    assert_eq!(delivery.payload(), Value::Null);
}

#[test]
fn an_issue_state_outside_the_webhook_vocabulary_fails_its_delivery() {
    let (mut state, request) = hooked_state();
    let mut issue = seeded_issue(&mut state, &request);
    issue.state = "merged".to_owned();
    let error =
        issues::opened(&mut Ctx::new(&mut state, &request, "admin", "tool"), &issue).unwrap_err();
    assert_eq!(
        error,
        PayloadError::Unknown {
            field: "issue.state",
            value: "merged".to_owned(),
        }
    );
    assert_unbuilt(&state, &error);
}

#[test]
fn an_issue_without_its_author_fails_its_delivery() {
    let (mut state, request) = hooked_state();
    let mut issue = seeded_issue(&mut state, &request);
    issue.user = None;
    let error =
        issues::opened(&mut Ctx::new(&mut state, &request, "admin", "tool"), &issue).unwrap_err();
    assert_eq!(
        error,
        PayloadError::Missing {
            field: "issue.user"
        }
    );
    assert_unbuilt(&state, &error);
}

#[test]
fn a_milestone_without_its_creator_fails_its_delivery() {
    let (mut state, request) = hooked_state();
    let issue = seeded_issue(&mut state, &request);
    let now = crate::time::now_utc();
    let milestone = types::NullableMilestone {
        closed_at: None,
        closed_issues: 0,
        created_at: now,
        creator: None,
        description: None,
        due_on: None,
        html_url: "http://127.0.0.1:9/admin/tool/milestone/1".to_owned(),
        id: 1,
        labels_url: "http://127.0.0.1:9/repos/admin/tool/milestones/1/labels".to_owned(),
        node_id: "MI_1".to_owned(),
        number: 1,
        open_issues: 1,
        state: types::NullableMilestoneState::Open,
        title: "v1".to_owned(),
        updated_at: now,
        url: "http://127.0.0.1:9/repos/admin/tool/milestones/1".to_owned(),
    };
    let error = issues::milestoned(
        &mut Ctx::new(&mut state, &request, "admin", "tool"),
        &issue,
        &milestone,
    )
    .unwrap_err();
    assert_eq!(
        error,
        PayloadError::Missing {
            field: "milestone.creator"
        }
    );
    assert_unbuilt(&state, &error);
}

#[test]
fn a_team_without_its_privacy_fails_its_delivery() {
    let (mut state, request) = hooked_state();
    let user = render::simple_user(&request, state.account("admin").unwrap());
    let repository = {
        let account = state.account("admin").unwrap();
        render::repository_for_pull(
            &request,
            account,
            account.repo("tool").unwrap(),
            Permission::default(),
        )
    };
    let pull = fixtures::seeded_pull_request(
        &mut state,
        &request,
        ("admin", "tool"),
        user,
        repository,
        2,
        PullRequestSeed::default(),
    )
    .unwrap()
    .data;
    let mut team = fixtures::requested_team(&mut state, &request, "acme", "reviewers").unwrap();
    team.privacy = None;
    let error = pull_request::review_requested_team(
        &mut Ctx::new(&mut state, &request, "admin", "tool"),
        &pull,
        &team,
    )
    .unwrap_err();
    assert_eq!(
        error,
        PayloadError::Missing {
            field: "team.privacy"
        }
    );
    assert_unbuilt(&state, &error);
}

#[test]
fn a_payload_that_cannot_be_built_answers_500_naming_the_field_and_value() {
    let response = crate::api::ApiResponse::from(PayloadError::Unknown {
        field: "issue.state",
        value: "merged".to_owned(),
    });
    assert_eq!(response.status, 500);
    let crate::api::Body::Json(body) = &response.body else {
        panic!("a JSON error body");
    };
    assert_eq!(
        body["message"],
        "ninot-github could not build the webhook payload: issue.state is \"merged\", which the \
         webhook schema does not allow"
    );
}
