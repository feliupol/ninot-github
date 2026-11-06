# ninot-github Python bindings

Stateful GitHub REST and Git server for tests. The binding uses real loopback
HTTP and Git; application clients need only their normal base URL and auth.

## Installation

From source (requires Rust and maturin):

```bash
pip install ./python
```

Or with maturin:

```bash
maturin develop
```

## Usage

```python
from ninot_github import Server

with Server() as server:
    server.add_user("alice")
    project = server.create_repository("alice", "project")
    token = server.add_token("alice", ["repo"])
    print(server.url)
```

Use the pytest fixture with `pytest -p ninot_github.pytest_plugin tests/`.

## Resource fixtures

Fixture state is owned by handles for the resource being seeded. The Python
package exports TypedDict models for sparse seed values:

```python
from ninot_github import PullRequestSeed, ReviewSeed, Server, UserSeed

with Server() as server:
    project = server.create_repository("admin", "project")
    pull = project.create_pull_request(9, PullRequestSeed(
        draft=False,
        head={"sha": "0123456789abcdef0123456789abcdef01234567"},
        base={"ref": "main"},
    ))
    pull.create_review(ReviewSeed(
        state="APPROVED",
        user=UserSeed(login="reviewer", type="Bot"),
        submitted_at="2026-01-02T03:04:05Z",
    ))
    server.start_profile()
    # The application under test makes ordinary HTTP requests to server.url.
    profile = server.stop_profile()
```

Pull-request handles own reviews, issue comments, review comments, and review
threads. Issue handles own issue comments. `Server.add_fault` injects a typed
fault into the real HTTP request path.

For GitHub App flows, call `project.create_installation(installation_id)`.
The server accepts JWT-shaped Bearer credentials at the App installation
endpoints and returns a repository-scoped installation token. Installation
permissions default to none; seed only the scope used by the scenario:

```python
from ninot_github import ActionsPublicKeySeed, ActionsSecretSeed, Server

with Server() as server:
    repository = server.create_repository("alice", "project", private=True)
    repository.create_installation(73, secrets_permission="write")
    repository.set_actions_public_key(
        ActionsPublicKeySeed(key_id="fixture-key", key="BASE64_PUBLIC_KEY")
    )
    existing = repository.create_actions_secret(
        ActionsSecretSeed(
            name="CI_TOKEN",
            encrypted_value="BASE64_CIPHERTEXT",
            key_id="fixture-key",
        )
    )
    # GET/list/PUT/DELETE secrets use the server's ordinary Actions REST routes.
    # Fixture inspection retains encrypted bytes without returning them from REST GETs.
    assert existing.encrypted_value
```

`secrets_permission` accepts `"read"`, `"write"`, or omission. Minted
installation tokens are restricted to that repository and explicit permission;
the built-in administrator token retains its normal full fixture access.

### Pull-request history

`GET /repos/{owner}/{repo}/issues/{number}/timeline` is built on each read from the pull
request's own lists, so it always agrees with them:

- `reviewed` from its submitted reviews (state in lower case, as GitHub's timeline has it),
  except a review holding only thread replies (`ReviewCommentSeed.in_reply_to_id`), which
  GitHub lists under `/reviews` but not in the timeline; `commented` from its issue comments;
- `committed` from the Git commits between its base and head, the same list
  `GET /pulls/{number}/commits` returns. A commit is placed where it reached the head
  branch; commits replaced by a force-push drop out, as on GitHub. A pull request whose head
  SHA is not in the repository's Git storage has no commits;
- stored events: `ready_for_review`, `convert_to_draft`, `review_requested`,
  `review_request_removed`, `head_ref_force_pushed`, `merged`, `closed`, `reopened`.

The REST and GraphQL actions record their events (merge, close/reopen through `PATCH`,
request/remove reviewers, ready/draft mutations), and a Git push to a pull request's head
branch moves its head and records a force-push when the old head is not an ancestor.

Fixtures seed the same history with explicit timestamps:

```python
from ninot_github import GitIdentitySeed, PullRequestSeed, Server, TimelineEventSeed

with Server() as server:
    repository = server.create_repository("admin", "project")
    alice = GitIdentitySeed(name="alice", email="alice@example.com", date="2026-01-02T09:00:00Z")
    first = repository.commit_file("feature", "a.txt", b"1", "First", author=alice)
    pull = repository.create_pull_request(1, PullRequestSeed(
        draft=True,
        created_at="2026-01-02T10:00:00Z",
        head={"ref": "feature", "sha": first.sha},
        files=[{"filename": "a.txt", "additions": 1}],
    ))
    pull.create_timeline_event(
        TimelineEventSeed(event="ready_for_review", created_at="2026-01-02T11:00:00Z")
    )
    rewritten = repository.commit_file(
        "rewrite", "a.txt", b"2", "Rewrite", author={**alice, "date": "2026-01-02T12:00:00Z"}
    )
    pull.push_head(rewritten.sha, pushed_at="2026-01-02T12:01:00Z")  # head_ref_force_pushed
```

- `create_timeline_event` applies the event to the pull request as the action would (it
  becomes ready, draft, merged, closed...). Events dated before the pull request's creation
  or its latest event, and transitions its state does not allow, are refused.
- `PullRequestSeed.merged_at` / `closed_at` close the pull request and write its `merged` and
  `closed` events. `merged` or a closed `state` without a timestamp are dated at the
  seeded `updated_at` (now by default). Contradictions (`merged: False` with `merged_at`, an open state with
  `closed_at`, a merged draft) are refused.
- `commit_file` takes `author`/`committer` identities; a commit on an open pull request's
  head branch moves the pull request and reaches it at its committer date (now without
  identities).
- `PullRequestSeed.files` serves `GET /pulls/{number}/files` and sets `changed_files`,
  `additions` and `deletions` unless they are seeded too (then they must agree).

Lists page as GitHub's do (`per_page` up to 100, `page`, and a `Link` header that keeps the
other query parameters). `GET /pulls` sorts by `created` (default, descending), `updated` or
`popularity` (ascending unless `direction=desc`); `sort=long-running` answers 501.
`GET /actions/runs` filters on `branch`, `event`, `status` and `head_sha`, newest first.

Repository CI state belongs to `Repository.create_workflow_run` and
`Commit.create_check_run`/`Commit.create_status`. Repository contents and
comparison state use real Git commits via `Repository.commit_file`; a fork's
`fork_from` copies refs and objects so cross-repository compare and pull-request
creation see the same history.
