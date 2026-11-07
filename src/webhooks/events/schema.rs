//! Builds every webhook payload from the fixture models and checks the pinned schema.
//!
//! The HTTP suite records the same deliveries. This test does not open a socket, so a payload
//! that the projector cannot satisfy fails here.

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use super::workflow_run;
use super::{Ctx, check_run, check_suite, create, delete, issue_comment, issues, pull_request};
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
        issues::opened(&mut ctx, &issue);
        for item in issues::issue_labels(&issue) {
            issues::labeled(&mut ctx, &issue, &item);
            issues::unlabeled(&mut ctx, &issue, &item);
        }
        issues::edited(&mut ctx, &issue, Some("Earlier"), Some(""));
        let mut closed = issue.clone();
        closed.state = "closed".to_owned();
        closed.closed_at = Some(render::timestamp(&crate::time::now()));
        issues::closed(&mut ctx, &closed);
        issues::reopened(&mut ctx, &issue);
        issues::assigned(&mut ctx, &issue, &user);
        issues::unassigned(&mut ctx, &issue, &user);
        let mut locked = issue.clone();
        locked.locked = true;
        issues::locked(&mut ctx, &locked);
        issues::unlocked(&mut ctx, &issue);

        pull_request::opened(&mut ctx, &pull);
        let mut changes = Map::new();
        changes.insert("title".to_owned(), json!({ "from": "Draft" }));
        changes.insert("body".to_owned(), json!({ "from": "" }));
        changes.insert(
            "base".to_owned(),
            json!({ "ref": { "from": "main" }, "sha": { "from": pull.base.sha } }),
        );
        pull_request::edited(&mut ctx, &pull, changes);
        let mut closed_pull = pull.clone();
        closed_pull.state = types::PullRequestState::Closed;
        closed_pull.closed_at = Some(render::timestamp(&crate::time::now()));
        pull_request::closed(&mut ctx, &closed_pull);
        pull_request::reopened(&mut ctx, &pull);
        pull_request::synchronize(&mut ctx, &pull, &pull.base.sha, &pull.head.sha, &user);
        for item in pull_request::pull_labels(&pull) {
            pull_request::labeled(&mut ctx, &pull, &item);
        }
        pull_request::review_requested_user(&mut ctx, &pull, &user);
        let team = fixtures::requested_team(ctx.state, &request, "acme", "reviewers").unwrap();
        pull_request::review_requested_team(&mut ctx, &pull, &team);

        let review = review(&request, &user, &pull);
        pull_request_review::submitted(&mut ctx, &pull, &review);
        let comment = review_comment(&request, &user, pull.number);
        pull_request_review_comment::created(&mut ctx, &pull, &comment);
        let issue_note = issue_comment(&request, &user, 1);
        issue_comment::created(&mut ctx, 1, &issue_note);
        issue_comment::edited(&mut ctx, 1, &issue_note, "old");
        issue_comment::deleted(&mut ctx, 1, &issue_note);
        ctx.state.add_pull_request("admin", "tool", 2, pull.clone());
        issue_comment::created(&mut ctx, 2, &issue_note);

        let check = fixtures::seeded_check_run(
            ctx.state,
            &request,
            "admin",
            "tool",
            sha,
            fixtures::CheckRunSeed::default(),
            0,
        )
        .unwrap();
        check_suite::requested(&mut ctx, &check);
        check_run::created(&mut ctx, &check);
        check_run::completed(&mut ctx, &check);
        // A rerequest follows a finished run, so its suite carries a conclusion.
        let finished = fixtures::seeded_check_run(
            ctx.state,
            &request,
            "admin",
            "tool",
            sha,
            fixtures::CheckRunSeed {
                status: fixtures::SeedField::Present("completed".to_owned()),
                conclusion: fixtures::SeedField::Present(Some("success".to_owned())),
                ..fixtures::CheckRunSeed::default()
            },
            1,
        )
        .unwrap();
        check_run::rerequested(&mut ctx, &finished);

        let run = {
            let account = ctx.state.account("admin").unwrap();
            let repo = account.repo("tool").unwrap();
            fixtures::seeded_workflow_run(
                &request,
                account,
                repo,
                fixtures::WorkflowRunSeed::default(),
            )
            .unwrap()
        };
        let job = fixtures::seeded_job(&request, &run, fixtures::JobSeed::default(), 0).unwrap();
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

        let status_model = {
            let account = ctx.state.account("admin").unwrap();
            let repo = account.repo("tool").unwrap();
            fixtures::seeded_commit_status(
                &request,
                account,
                repo,
                sha,
                fixtures::CommitStatusSeed::default(),
                0,
            )
            .unwrap()
        };
        status::commit_status(&mut ctx, sha, &status_model);

        let published = sample_release(user);
        release::deliver(&mut ctx, "created", &published, None);
        release::deliver(&mut ctx, "published", &published, None);
        let mut prerelease = published.clone();
        prerelease.prerelease = true;
        release::deliver(&mut ctx, "prereleased", &prerelease, None);
        release::deliver(&mut ctx, "unpublished", &published, None);
        release::deliver(&mut ctx, "deleted", &published, None);
        let mut edited = Map::new();
        edited.insert("name".to_owned(), json!({ "from": "Draft" }));
        edited.insert("body".to_owned(), json!({ "from": "" }));
        release::deliver(&mut ctx, "edited", &published, Some(edited));

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
    let now = render::timestamp(&crate::time::now());
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
    let now = fixtures::current_timestamp();
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
    let now = fixtures::current_timestamp();
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
    let now = fixtures::current_timestamp();
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
