"""Tests for the Python bindings."""

import json
import urllib.error
import urllib.request

import pytest

from ninot_github import (
    FaultRule,
    IssueCommentSeed,
    JobSeed,
    PullRequestSeed,
    ReviewCommentSeed,
    ReviewSeed,
    Server,
    TeamSeed,
    UserSeed,
    WorkflowRunSeed,
)


def test_zen() -> None:
    """Test that /zen endpoint returns 200."""
    server = Server()
    response = urllib.request.urlopen(f"{server.url}/zen")
    assert response.status == 200
    server.stop()


def test_authenticated_request() -> None:
    """Test user authentication and token creation."""
    server = Server()
    login = "testuser"
    user = server.add_user(login)
    scopes = ["repo", "gist"]
    token = user.add_token(scopes)

    request = urllib.request.Request(
        f"{server.url}/user",
        headers={"Authorization": f"token {token}"},
    )
    response = urllib.request.urlopen(request)
    assert response.status == 200
    user = json.loads(response.read())
    assert user["login"] == login
    found = server.user(login)
    assert found is not None
    assert found.login == login
    assert server.user("nobody") is None
    server.stop()


def test_fixture(ninot_github: Server) -> None:
    """Test that the pytest fixture works."""
    assert ninot_github.url.startswith("http")
    assert ninot_github.admin_token


def test_stop_closes_server() -> None:
    """Test that stop() makes the server refuse connections."""
    server = Server()
    url = server.url
    server.stop()
    with pytest.raises(urllib.error.URLError):
        urllib.request.urlopen(f"{url}/zen")


def test_applied_faults() -> None:
    """Test that applied_faults() returns a list of dicts with expected keys."""
    server = Server()

    fault_rule: FaultRule = {
        "target": {"path_prefix": "/zen"},
        "fault": {"latency_ms": 100},
        "probability": 1.0,
    }
    server.add_fault(fault_rule)

    urllib.request.urlopen(f"{server.url}/zen")

    faults = server.applied_faults()
    assert len(faults) == 1
    assert faults[0]["method"] == "GET"
    assert faults[0]["path"] == "/zen"
    assert faults[0]["fault"] == {"latency_ms": 100}
    assert "target" in faults[0]

    server.stop()


def test_pull_request_review_and_comment_state_is_served_over_http() -> None:
    """The fixture seeds real API reads and records requests over the socket."""
    server = Server()
    repository = server.add_repository("admin", "project")
    pull = repository.add_pull_request(
        9,
        PullRequestSeed(
            draft=True,
            state="open",
            mergeable=False,
            base={"ref": "main"},
            head={"sha": "0123456789012345678901234567890123456789"},
        ),
    )
    pull.add_review(ReviewSeed(state="APPROVED", user=UserSeed(login="reviewer", type="Bot")))
    pull.add_issue_comment(
        IssueCommentSeed(body="Looks good", user=UserSeed(login="reviewer", type="Bot"))
    )
    pull.add_review_comment(
        ReviewCommentSeed(
            path="src/main.rs",
            line=7,
            body="Please check this.",
            user=UserSeed(login="reviewer", type="Bot"),
        )
    )
    server.start_profile()
    headers = {"Authorization": f"token {server.admin_token}"}
    pull = json.loads(
        urllib.request.urlopen(
            urllib.request.Request(f"{server.url}/repos/admin/project/pulls/9", headers=headers)
        ).read()
    )
    reviews = json.loads(
        urllib.request.urlopen(
            urllib.request.Request(
                f"{server.url}/repos/admin/project/pulls/9/reviews", headers=headers
            )
        ).read()
    )
    comments = json.loads(
        urllib.request.urlopen(
            urllib.request.Request(
                f"{server.url}/repos/admin/project/issues/9/comments", headers=headers
            )
        ).read()
    )
    review_comments = json.loads(
        urllib.request.urlopen(
            urllib.request.Request(
                f"{server.url}/repos/admin/project/pulls/9/comments", headers=headers
            )
        ).read()
    )
    assert pull["draft"] is True
    assert pull["base"]["ref"] == "main"
    assert pull["head"]["sha"] == "0123456789012345678901234567890123456789"
    assert reviews[0]["user"]["type"] == "Bot"
    assert comments[0]["body"] == "Looks good"
    assert review_comments[0]["line"] == 7
    profile = server.stop_profile()
    assert profile["requests"] == 4
    assert [hit["status"] for hit in profile["hits"]] == [200, 200, 200, 200]
    server.stop()


def test_git_and_workflow_fixture_apis_serve_real_http_state() -> None:
    """Git commits, fork refs, and Actions fixture rows are visible over REST."""
    with Server() as server:
        server.add_user("upstream")
        alice = server.add_user("alice")
        token = alice.add_token(["repo", "public_repo"])
        upstream = server.add_repository(
            "upstream", "project", description="upstream", language="Rust"
        )
        head_sha = upstream.commit_file("topic", "README.md", b"topic content\n", "topic").sha
        fork = server.add_repository("alice", "project-fork")
        fork.fork_from(upstream)
        headers = {"Authorization": f"token {token}"}
        fork.add_workflow_run(
            WorkflowRunSeed(
                id=42,
                name="CI",
                head_branch="topic",
                head_sha=head_sha,
                event="push",
                status="completed",
                conclusion="success",
                run_number=1,
                display_title="CI",
                path=".github/workflows/ci.yml",
                workflow_id=7,
            ),
            [JobSeed(id=91, name="tests", status="completed", conclusion="success")],
        )
        server.start_profile()
        content_request = urllib.request.Request(
            f"{server.url}/repos/alice/project-fork/contents/README.md?ref=topic", headers=headers
        )
        content = json.loads(urllib.request.urlopen(content_request).read())
        assert content["encoding"] == "base64"
        assert content["content"].strip() == "dG9waWMgY29udGVudAo="
        runs_request = urllib.request.Request(
            f"{server.url}/repos/alice/project-fork/actions/runs?branch=topic&event=push&status=completed",
            headers=headers,
        )
        runs = json.loads(urllib.request.urlopen(runs_request).read())
        assert runs["workflow_runs"][0]["head_sha"] == head_sha
        jobs_request = urllib.request.Request(
            f"{server.url}/repos/alice/project-fork/actions/runs/42/jobs", headers=headers
        )
        jobs = json.loads(urllib.request.urlopen(jobs_request).read())
        assert jobs["jobs"][0]["name"] == "tests"
        assert server.stop_profile()["requests"] == 3


def test_team_review_requests_are_searchable_by_member() -> None:
    """A pull request requesting a team appears in its members' review-requested search."""
    with Server() as server:
        server.add_user("me")
        org = server.add_org("org", ["me"])
        org.add_team("reviewers", ["me"])
        assert org.runner(1) is None
        repository = server.add_repository("org", "project")
        repository.add_pull_request(
            1, PullRequestSeed(requested_teams=[TeamSeed(slug="reviewers")])
        )
        me = server.add_user("me")
        token = me.add_token(["repo"])
        request = urllib.request.Request(
            f"{server.url}/search/issues?q=is%3Apr+review-requested%3A%40me",
            headers={"Authorization": f"token {token}"},
        )
        found = json.loads(urllib.request.urlopen(request).read())
        assert [item["number"] for item in found["items"]] == [1]


def test_org_plan_is_shown_to_owners_and_limits_private_repositories() -> None:
    """A Free plan shows on the organization and refuses rulesets on its private repositories."""
    with Server() as server:
        token = server.add_user("me").add_token(["repo"])
        org = server.add_org("org", ["me"])
        server.add_repository("org", "project", private=True)
        org.set_plan("free")

        def get(path: str) -> urllib.request.Request:
            return urllib.request.Request(
                f"{server.url}{path}", headers={"Authorization": f"token {token}"}
            )

        found = json.loads(urllib.request.urlopen(get("/orgs/org")).read())
        assert found["plan"]["name"] == "free"
        with pytest.raises(urllib.error.HTTPError) as refused:
            urllib.request.urlopen(get("/repos/org/project/rulesets"))
        assert refused.value.code == 403

        org.set_plan(None)
        assert json.loads(urllib.request.urlopen(get("/repos/org/project/rulesets")).read()) == []
        with pytest.raises(ValueError, match="unknown plan"):
            org.set_plan("platinum")


def test_add_release_seeds_a_release_with_assets() -> None:
    """Test that a seeded release is served by the releases API."""
    server = Server()
    admin = {"Authorization": f"token {server.admin_token}"}
    repository = server.add_repository("admin", "tool", default_branch="main")
    release_id = repository.add_release({
        "tag_name": "v1",
        "draft": True,
        "assets": [{"name": "a.txt", "content_base64": "aGk="}],
    })
    request = urllib.request.Request(
        f"{server.url}/repos/admin/tool/releases/{release_id}", headers=admin
    )
    release = json.loads(urllib.request.urlopen(request).read())
    assert release["tag_name"] == "v1"
    assert release["draft"] is True
    assert release["assets"][0]["size"] == 2
    with pytest.raises(ValueError, match="already exists"):
        repository.add_release({"tag_name": "v1"})
    server.stop()


def test_app_credentials_installations_and_deliveries_are_seeded_through_models() -> None:
    """Test that App tokens, OAuth apps, installations and deliveries need no HTTP state API."""
    server = Server()
    alice = server.add_user("alice")
    repository = server.add_repository("alice", "tool", default_branch="main")
    repository.add_installation(7, "read", 9)
    server.add_oauth_app("client", "secret", 9)
    token = alice.add_app_token(9)
    assert token.startswith("ghu_")
    request = urllib.request.Request(
        f"{server.url}/user/installations", headers={"Authorization": f"token {token}"}
    )
    installations = json.loads(urllib.request.urlopen(request).read())["installations"]
    assert [installation["id"] for installation in installations] == [7]
    assert server.deliveries() == []
    assert server.deliveries("push") == []
    with pytest.raises(urllib.error.HTTPError) as missing:
        urllib.request.urlopen(f"{server.url}/_ninot/faults")
    assert missing.value.code == 404
    server.stop()
