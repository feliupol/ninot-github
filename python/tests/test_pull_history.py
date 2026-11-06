"""Pull-request history seeded through the Python fixture API and read over HTTP."""

import json
import urllib.request

import pytest

from ninot_github import (
    GitIdentitySeed,
    PullRequestFileSeed,
    PullRequestSeed,
    ReviewSeed,
    Server,
    TimelineEventSeed,
    UserSeed,
)


def _get(server: Server, path: str) -> tuple[object, dict[str, str]]:
    """GET `path` as the administrator and return the JSON body and headers."""
    request = urllib.request.Request(
        f"{server.url}{path}", headers={"Authorization": f"token {server.admin_token}"}
    )
    with urllib.request.urlopen(request) as response:
        return json.loads(response.read()), dict(response.headers)


def _alice(date: str) -> GitIdentitySeed:
    """Alice as a commit author at `date`."""
    return GitIdentitySeed(name="alice", email="alice@example.com", date=date)


def test_seeded_draft_history_and_force_push_read_back_in_order() -> None:
    """Draft changes, a force-push, a review and the merge come back in time order."""
    with Server() as server:
        repository = server.create_repository("admin", "project")
        base = repository.commit_file(
            "main", "base.txt", b"base", "Base", author=_alice("2026-01-01T00:00:00Z")
        )
        first = repository.commit_file(
            "feature", "a.txt", b"1", "First", author=_alice("2026-01-02T09:00:00Z")
        )
        pull = repository.create_pull_request(
            1,
            PullRequestSeed(
                draft=True,
                created_at="2026-01-02T10:00:00Z",
                user=UserSeed(login="alice"),
                base={"ref": "main", "sha": base.sha},
                head={"ref": "feature", "sha": first.sha},
                files=[PullRequestFileSeed(filename="a.txt", additions=1)],
            ),
        )
        pull.create_timeline_event(
            TimelineEventSeed(event="ready_for_review", created_at="2026-01-02T11:00:00Z")
        )
        pull.create_timeline_event(
            TimelineEventSeed(
                event="review_requested",
                created_at="2026-01-02T11:00:00Z",
                requested_reviewer=UserSeed(login="bob"),
            )
        )
        rewritten = repository.commit_file(
            "rewrite", "a.txt", b"2", "Rewrite", author=_alice("2026-01-02T12:00:00Z")
        )
        pull.push_head(rewritten.sha, pushed_at="2026-01-02T12:01:00Z")
        pull.create_review(
            ReviewSeed(
                state="APPROVED", user=UserSeed(login="bob"), submitted_at="2026-01-02T13:00:00Z"
            )
        )
        pull.create_timeline_event(
            TimelineEventSeed(
                event="merged", created_at="2026-01-02T14:00:00Z", actor=UserSeed(login="bob")
            )
        )
        pull.create_timeline_event(
            TimelineEventSeed(
                event="closed", created_at="2026-01-02T14:00:00Z", actor=UserSeed(login="bob")
            )
        )

        timeline, _ = _get(server, "/repos/admin/project/issues/1/timeline")
        commits, _ = _get(server, "/repos/admin/project/pulls/1/commits")
        files, _ = _get(server, "/repos/admin/project/pulls/1/files")
        read, _ = _get(server, "/repos/admin/project/pulls/1")

    assert isinstance(timeline, list)
    assert [entry["event"] for entry in timeline] == [
        "ready_for_review",
        "review_requested",
        "committed",
        "head_ref_force_pushed",
        "reviewed",
        "merged",
        "closed",
    ]
    assert timeline[3]["commit_id"] == rewritten.sha
    assert timeline[3]["created_at"] == "2026-01-02T12:01:00Z"
    assert timeline[4]["state"] == "approved"
    assert isinstance(commits, list)
    assert [commit["sha"] for commit in commits] == [rewritten.sha]
    assert commits[0]["commit"]["author"]["date"] == "2026-01-02T12:00:00Z"
    assert isinstance(files, list)
    assert [file["filename"] for file in files] == ["a.txt"]
    assert isinstance(read, dict)
    assert (read["state"], read["merged"], read["merged_at"], read["closed_at"]) == (
        "closed",
        True,
        "2026-01-02T14:00:00Z",
        "2026-01-02T14:00:00Z",
    )


def test_merged_at_seed_closes_the_pull_request() -> None:
    """A merged_at seed sets merged, closed_at and the merged and closed events."""
    with Server() as server:
        repository = server.create_repository("admin", "project")
        repository.create_pull_request(
            2,
            PullRequestSeed(created_at="2026-02-01T00:00:00Z", merged_at="2026-02-02T00:00:00Z"),
        )
        read, _ = _get(server, "/repos/admin/project/pulls/2")
        timeline, _ = _get(server, "/repos/admin/project/issues/2/timeline")

    assert isinstance(read, dict)
    assert (read["state"], read["merged"], read["closed_at"]) == (
        "closed",
        True,
        "2026-02-02T00:00:00Z",
    )
    assert isinstance(timeline, list)
    assert [entry["event"] for entry in timeline] == ["merged", "closed"]


def test_out_of_order_timeline_event_is_refused() -> None:
    """Events must not predate the pull request or its latest event."""
    with Server() as server:
        repository = server.create_repository("admin", "project")
        pull = repository.create_pull_request(3, PullRequestSeed(created_at="2026-02-01T00:00:00Z"))
        with pytest.raises(ValueError, match="earlier than the pull request"):
            pull.create_timeline_event(
                TimelineEventSeed(event="convert_to_draft", created_at="2026-01-01T00:00:00Z")
            )


def test_timeline_pages_beyond_one_hundred_entries() -> None:
    """The timeline pages at 100 entries with a next link that keeps per_page."""
    with Server() as server:
        repository = server.create_repository("admin", "project")
        pull = repository.create_pull_request(4, PullRequestSeed(created_at="2026-03-01T00:00:00Z"))
        for index in range(101):
            pull.create_issue_comment({
                "body": f"comment {index}",
                "created_at": f"2026-03-02T00:{index // 60:02}:{index % 60:02}Z",
            })
        first, headers = _get(server, "/repos/admin/project/issues/4/timeline?per_page=100")
        second, _ = _get(server, "/repos/admin/project/issues/4/timeline?per_page=100&page=2")

    assert isinstance(first, list)
    assert len(first) == 100
    assert 'per_page=100&page=2>; rel="next"' in headers["link"]
    assert isinstance(second, list)
    assert [entry["body"] for entry in second] == ["comment 100"]
