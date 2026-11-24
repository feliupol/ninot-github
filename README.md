# ninot-github

An in-memory, stateful fake of the GitHub REST API, the GraphQL API and git hosting, for
end-to-end tests that must not touch the network or a real account.

A mock returns canned responses. This fake keeps the accounts, repositories, git history,
issues and runs you create, and serves later requests from that state. Creating a
repository, pushing a commit and reading the file back through the contents API all see the
same object. It can also misbehave on purpose, so a client can be tested against latency,
error statuses and dropped connections.

The REST surface comes from GitHub's pinned OpenAPI description. An operation with no
handler answers `501` and `Not implemented by ninot-github: {operationId}`.

## Quick start

[`examples/quickstart.rs`](examples/quickstart.rs) starts a server, adds a user and a token,
creates a repository, commits a file and reads it back. `commit_file` shells out to `git`;
the example prints `skip: git is not installed` when `git` is absent.

```rust
let server = ninot_github::Server::start().expect("start the fake");
let alice = server.add_user("alice");
let token = alice.add_token(&["repo"]);
let repo = server
    .create_repository("alice", "hello", false, None, None, "main")
    .expect("create the repository");
repo.commit_file("main", Path::new("README.md"), b"hello\n", "Initial commit")
    .expect("commit README.md");

let body = reqwest::blocking::Client::new()
    .get(format!("{}/repos/alice/hello/contents/README.md", server.url()))
    .header("Authorization", format!("token {token}"))
    .header("Accept", "application/vnd.github.raw")
    .send()
    .expect("GET the file")
    .error_for_status()
    .expect("contents status")
    .text()
    .expect("contents body");
assert_eq!(body, "hello\n");
```

From another process, `ninot-github serve [--port PORT] [--tls]` prints the URL, an admin
token and, with `--tls`, the CA certificate path.

## Object model

```text
Server
├── User
├── Org
│   ├── Team
│   └── Runner
└── Repository
    ├── Issue
    ├── PullRequest
    │   └── Review
    ├── Commit
    ├── WorkflowRun
    ├── WorkflowJob
    ├── ActionsSecret
    └── Project
```

`Server` starts the fake. `add_user`, `add_org` and `create_repository` return handles;
`user`, `org`, `repository` and `runner` look up ones that already exist. A repository
handle creates and fetches its issues, pull requests, commits, workflow runs, jobs, Actions
secrets and projects. The same types are re-exported from the crate root, except
`WorkflowJob` and `ActionsSecret`, which live in [`server`](src/server/mod.rs).

## Git hosting

Each repository is a real git repository served over smart HTTP at
`{url}/{owner}/{repo}.git`. Authenticate with Basic auth, username `x-access-token` and the
token as the password. [`Repository::commit_file`](src/server/seed.rs) writes a commit through
`git` on a fixture path when the test does not want to drive a work tree.

## Fault injection

[`Server::add_fault`](src/server/mod.rs) installs a [`Rule`](src/faults.rs) of a [`Target`](src/faults.rs)
and a [`Fault`](src/faults.rs). Targets are every request, one `operationId`, a git service
(`upload-pack` or `receive-pack`), or a path prefix. Faults are a delay, an HTTP status, a
secondary rate limit, a dropped connection, or a truncated body. `clear_faults` removes the
rules, `seed_faults` restarts the random sequence, and `applied_faults` lists what fired.
See [`examples/faults.rs`](examples/faults.rs).

A `Fault::Respond` rule answers with a status, headers and body of its own, such as a `200` with an
HTML body or a `429` with `Retry-After`:

```json
{"target": "any", "fault": {"respond": {"status": 429, "headers": {"retry-after": "7"}, "body": "{\"message\":\"slow down\"}"}}}
```

`content_type` defaults to `application/json; charset=utf-8`.

## Fixture state

Seed and inspect the fake through the Rust handles of `Server` or the `ninot_github` Python
models; the simulated service's network surface carries only GitHub's own protocol. Each
fixture operation has both forms: `Server::add_fault` and `add_fault`, `add_user` and
`User::add_token` / `User::add_app_token`, `add_org`, `add_repository`,
`Repository::add_app_installation` / `add_installation(..., app_id=...)`,
`Repository::add_release` and `ReleaseSeed`, `Server::add_oauth_app`, and `Server::deliveries`.

## GitHub Apps and OAuth

An App JWT lists its installations (`GET /app/installations`, filtered by the `iss` claim, with
`since`) and finds one by repository. `GET /installation/repositories` serves an installation
access token. A user access token, issued by the OAuth flow below or by
`User::add_app_token`, lists the
installations of its app that the user owns, administers or collaborates on
(`GET /user/installations`, `GET /user/installations/{id}/repositories`); any other token gets `403`.

The OAuth web flow lives at the server root, as it does on github.com. Register the app's
credentials first with `Server::add_oauth_app`.

- `GET /login/oauth/authorize?client_id=...&redirect_uri=...&state=...` approves at once as the
  user named by `login` (the `admin` user when absent) and redirects with `302` to
  `redirect_uri?code=...&state=...`. `redirect_uri` is required.
- `POST /login/oauth/access_token` with `client_id`, `client_secret`, `code` and optionally
  `redirect_uri` (form, JSON or query) answers `access_token`, `scope` and `token_type` as a
  form-encoded body, or as JSON with `Accept: application/json`. A code is single-use. Failures
  answer `200` with `error` (`incorrect_client_credentials`, `bad_verification_code`,
  `redirect_uri_mismatch`), `error_description` and `error_uri`. Tokens do not expire and there
  is no refresh token.

To point a client at the fake, set its OAuth base URL to the server URL (for example
`http://127.0.0.1:PORT`), so that it requests `{url}/login/oauth/authorize` and
`{url}/login/oauth/access_token`; point its API base URL at `{url}` or `{url}/api/v3`.

## Rate limits and the clock

The default limits are GitHub's: core 5000 per hour authenticated and 60 anonymous, search
30 and 10 per minute, GraphQL 5000 and 0 per hour, 100 requests in flight, and 80
content-creating REST requests per minute. Set them with
`Options { rate_limits, .. }`. A client over the concurrent or content-creation cap gets a
secondary rate limit (`403` and `retry-after`). [`Server::advance_clock`](src/server/mod.rs)
expires windows without waiting.

## Webhooks

`POST /repos/{owner}/{repo}/hooks` creates a hook. Deliveries are POSTed to its URL and, when
the hook has a secret, signed with `X-Hub-Signature-256` (HMAC-SHA256, `sha256=` prefix).
[`Server::deliveries`](src/server/mod.rs) returns them; `wait_for_deliveries` waits until enough
have finished.

## GitHub CLI over TLS

`serve --tls` (or `Options { tls: true, .. }`) serves HTTPS from a fresh local CA.
`Server::ca_cert_path` is that certificate. On Unix, `gh` trusts it through the environment:

```sh
GH_HOST=127.0.0.1:PORT GH_ENTERPRISE_TOKEN=TOKEN \
  SSL_CERT_FILE=CA GIT_SSL_CAINFO=CA gh repo list
```

On Windows `gh` reads the system certificate store and ignores `SSL_CERT_FILE`. The `gh_cli`
tests are Unix-only for that reason.

## GraphQL

`POST /graphql` and `POST /api/graphql` serve the published schema and require a token. A
field listed in the resolver table is computed. Any other field reads the same-named key of
its parent object and, when that key is absent, returns
`Not implemented by ninot-github: Type.field`.

## Actions runner

A registered self-hosted runner can take a job queued with
[`Repository::queue_script_job`](src/server/seed.rs). Registration uses the registration-token
endpoint, and the runner's own calls are served under `/_services/`. The runner drops the
port of its `--url`, so it has to reach the fake on port 443. Details, including
`NINOT_RUNNER_E2E`, are in [docs/actions-runner.md](docs/actions-runner.md).
[`examples/orgs_and_runners.rs`](examples/orgs_and_runners.rs) creates an organization, a
team and a runner.

## Repository setup

A repository administrator can set its topics, its Actions permissions, deployment
environments and rulesets. An environment keeps user reviewers, `prevent_self_review`, the
deployment branch policy, custom branch and tag policies, and secrets. Its secrets key is
derived from the repository and environment ids, so `gh secret set --env` can seal values to
it; nothing can decrypt them. Rulesets are stored and served back, not enforced on push. Team
reviewers are refused with `422`.

`GET /repos/{owner}/{repo}/git/refs[/{prefix}]`, which the pinned description no longer lists,
answers like list-matching-refs: `409 Git Repository is empty.` for a repository without refs.

[`Org::set_plan`](src/server/seed/plan.rs) gives an organization a billing plan, shown to its
owners in `GET /orgs/{org}`. On its private repositories, `free` refuses rulesets with `403
Upgrade to GitHub Pro or make this repository public to enable this feature.`, and every plan
but `enterprise` refuses required reviewers with a `422` naming the billing plan; environments
themselves exist on every plan. Public repositories, and organizations without a plan, have no
limits.

## Python package

[`python/`](python/) is a PyO3 binding of the same handles (`Server`, `User`, `Org`, `Team`,
`Runner`, `Repository` and the repository resources). `pip install ./python` or
`maturin develop` builds it. Usage and the pytest plugin are in
[python/README.md](python/README.md).

<!-- doc-gen completion -->

## Status

REST operations: 286 / 1231 (23.2%).

| Tag | Implemented | Total | % |
| --- | ---: | ---: | ---: |
| actions | 80 | 200 | 40.0% |
| activity | 0 | 34 | 0.0% |
| agent-tasks | 0 | 5 | 0.0% |
| agents | 0 | 30 | 0.0% |
| apps | 6 | 37 | 16.2% |
| billing | 0 | 13 | 0.0% |
| campaigns | 0 | 5 | 0.0% |
| checks | 12 | 12 | 100.0% |
| classroom | 0 | 6 | 0.0% |
| code-quality | 0 | 4 | 0.0% |
| code-scanning | 0 | 25 | 0.0% |
| code-security | 0 | 20 | 0.0% |
| codes-of-conduct | 0 | 2 | 0.0% |
| codespaces | 0 | 48 | 0.0% |
| copilot | 0 | 31 | 0.0% |
| copilot-spaces | 0 | 28 | 0.0% |
| credentials | 0 | 1 | 0.0% |
| dependabot | 0 | 25 | 0.0% |
| dependency-graph | 0 | 5 | 0.0% |
| emojis | 0 | 1 | 0.0% |
| gists | 2 | 20 | 10.0% |
| git | 13 | 13 | 100.0% |
| gitignore | 0 | 2 | 0.0% |
| hosted-compute | 0 | 6 | 0.0% |
| interactions | 0 | 16 | 0.0% |
| issues | 40 | 61 | 65.6% |
| licenses | 0 | 3 | 0.0% |
| markdown | 0 | 2 | 0.0% |
| meta | 3 | 5 | 60.0% |
| migrations | 0 | 22 | 0.0% |
| oidc | 0 | 8 | 0.0% |
| orgs | 11 | 116 | 9.5% |
| packages | 0 | 27 | 0.0% |
| private-registries | 0 | 6 | 0.0% |
| projects | 0 | 26 | 0.0% |
| pulls | 27 | 34 | 79.4% |
| rate-limit | 1 | 1 | 100.0% |
| reactions | 12 | 15 | 80.0% |
| repos | 46 | 203 | 22.7% |
| search | 7 | 7 | 100.0% |
| secret-scanning | 0 | 17 | 0.0% |
| security-advisories | 0 | 10 | 0.0% |
| teams | 15 | 32 | 46.9% |
| users | 11 | 47 | 23.4% |

GitHub CLI leaf commands: 79 / 204 (38.7%).

GraphQL resolvers: 76 / 5577 schema fields (1.4%). The published schema is served. A field without a resolver reads the same-named key of its parent object and returns the error `Not implemented by ninot-github: Type.field` when that key is absent.

Python bindings: 74 / 84 public methods.

| Type | Exposed | Total | Missing |
| --- | ---: | ---: | --- |
| ActionsSecret | 3 | 3 | none |
| Commit | 3 | 3 | none |
| Issue | 2 | 2 | none |
| Org | 5 | 5 | none |
| Project | 3 | 3 | none |
| PullRequest | 8 | 8 | none |
| Repository | 17 | 24 | `add_actions_cache`, `add_app_installation`, `add_app_installation_with_secrets_permission`, `add_artifact`, `add_installation_with_secrets_permission`, `commit_file_by`, `queue_script_job` |
| Review | 2 | 2 | none |
| Runner | 2 | 2 | none |
| Server | 21 | 24 | `hits`, `wait`, `wait_for_deliveries` |
| Team | 2 | 2 | none |
| User | 4 | 4 | none |
| WorkflowRun | 2 | 2 | none |

<!-- end-doc-gen -->

## Build and test

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --workspace -- -D warnings -A unfulfilled_lint_expectations
CLIPPY_CONF_DIR=tools/untyped-json cargo clippy -p ninot-github --lib --bins -- \
  -D clippy::disallowed_types -D clippy::disallowed_macros -D clippy::disallowed_methods \
  -D unfulfilled_lint_expectations
cargo test --all-targets --workspace --exclude ninot-github-python
cargo build --examples
cargo xtask completion check
```

`cargo xtask` is the maintainer alias (`completion check|update`,
`gh-coverage report|check|update`, `example-shapes report|check|update`). The Python package is tested with
`maturin develop -m python/Cargo.toml` and `pytest -q python/tests`.

`cargo xtask gh-coverage check` reads what the last `cargo test --test gh_cli` run exercised.
It fails when that differs from `tests/gh_coverage.txt` (`update` rewrites the file). It also
fails when an operation in `IMPLEMENTED` is neither answered successfully in a `gh_cli` test nor
listed in `tests/gh_coverage_exemptions.txt` as `operationId: reason`. An exemption for an
operation that is exercised, or no longer implemented, fails the check too, so the list only
shrinks. A new handler therefore needs a `gh_cli` test, or an exemption that says why, for
example that gh never calls the endpoint.

`cargo xtask example-shapes check` compares the successful JSON responses the last
`cargo test --all-targets --workspace` run logged (the tests enable the `response-log` feature,
which writes them under `target/tmp/example-shapes/`) with the examples the spec documents for
each implemented operation: same keys, JSON types and nesting, and enum values from the schema.
Optional keys count as missing only when every logged body omits them. It fails when the
findings differ from `tests/example_shapes.txt`, so the list of mismatches only shrinks; `update`
rewrites the file and `report` prints them per operation.

## Behaviour references

The OpenAPI description fixes paths, parameters and response shapes. Anything it leaves open
needs a source, because it is GitHub's behaviour and not ours to invent. That covers which of the
documented statuses a handler returns, derived fields (such as `mergeable_state` or
`reviewDecision`), ordering, permission rules and validation messages. Code that decides one of
these has a comment with a `Source:` line naming where the behaviour comes from:

- a docs.github.com page, with its anchor:
  `Source: <https://docs.github.com/en/rest/pulls/pulls#merge-a-pull-request>`
- an OpenAPI operation and status: `Source: OpenAPI pulls/merge 405`
- the GitHub CLI at a pinned tag, for behaviour a client depends on:
  `Source: cli/cli v2.101.0 pkg/cmd/pr/merge/http.go:89`

Put the comment on the item, or on the statement when the item has more than one such choice.
If you cannot find a source, do not invent the behaviour. Leave it out, or ask in the pull
request.

## Vendored references

The fake builds on copies of GitHub's published descriptions. Each is pinned to an upstream
commit or content digest, recorded in a pin file next to it:

| Reference | Upstream | Pin |
| --- | --- | --- |
| `spec/api.github.com.json` | github/rest-api-description | `spec/SOURCE` |
| `spec/schema.docs.graphql` | docs.github.com public GraphQL schema | `spec/GRAPHQL_SOURCE` (SHA-256) |
| `spec/webhooks/` | octokit/webhooks payload schemas | `spec/webhooks/SOURCE` |
| `tests/fixtures/octokit/` | octokit/fixtures api.github.com recordings | `tests/fixtures/octokit/SOURCE` |
| gh CLI version | the installed `gh` | header of `tests/gh_coverage.txt` |

`cargo references update` refreshes all of them. The tool lives in `tools/references`, and does
not depend on this crate, so it still runs when a spec bump breaks the build. Each upstream
source is an entry in `tools/references/sources.json`. After the sources, it re-pins the gh
version when the installed gh differs (running the `gh_cli` tests, then
`cargo xtask gh-coverage update`), and regenerates `completion.json` and the README status with
`cargo xtask completion update`. It prints a summary of the pins that moved and the changed
files (`--summary FILE` also writes it). A failed regeneration, such as a spec change the
handlers no longer compile against, is listed in the summary and makes it exit 3, with the new
pins still written. `cargo references check` writes nothing and exits 1 if a pin would move or
the installed gh differs from the pinned one. It also reports the latest cli/cli release.

No GitHub credentials are needed: the tool uses `git ls-remote`, a tree-only `git fetch` to
list globbed paths, and raw.githubusercontent.com. An optional `REFERENCES_GITHUB_COM_TOKEN`
raises the anonymous rate limits and is sent to github.com hosts only. It never reads
`GITHUB_TOKEN`, which in Forgejo Actions is the forge's token.

`.forgejo/workflows/update-references.yml` runs `cargo references publish` every Tuesday at
03:40 UTC, and on demand (`workflow_dispatch`, with a `dry_run` input that commits locally and
prints the summary). When a pin moved, `publish` commits as `forgejo-actions[bot]` with the
summary, force-pushes `chore/update-references` and opens or updates its pull request to `main`.
It reads the forge from the standard Actions variables (`GITHUB_API_URL` or
`GITHUB_SERVER_URL`, `GITHUB_REPOSITORY`, `GITHUB_REF`) and refuses any ref other than `main`.
A failed regeneration still opens the pull request, then fails the run. It pushes with the
`REFERENCES_BOT_TOKEN` secret, so CI runs on that pull request; events from the job's own token
would start no workflow.

To add a reference, add an entry to `tools/references/sources.json`:

- a git source names `repository` (`owner/name` on github.com), `ref` (such as
  `refs/heads/main`, or a commit to pin by hand), the `files` to copy (paths relative to
  `from`; `*` matches within one path segment, and `to` renames, with `{1}` for what the first
  `*` matched), and the `into` directory. `replace` clears that directory first, and
  `follow_refs` also copies every JSON schema the files reference through `$ref`;
- a URL source names `url` and the `to` file, and is pinned by its SHA-256;
- `pin` is the pin file and its template: `{repository}`, `{commit}` and `{from}` for git, `{url}`
  and `{sha256}` for URLs. A github.com `blob` or `tree` URL at a commit shortens to
  `<owner>/<repo>@<commit>` in the summary;
- `forbid` lists case-insensitive patterns that no written file may contain.

The check, the update and the weekly job pick it up without changes. Code generated from a
reference belongs in `build.rs` or an xtask the tool already runs.

## Untyped JSON

Non-test code in `src/` works with typed data, not with `serde_json::Value`. The second clippy
command above bans, through `tools/untyped-json/clippy.toml`:

- `disallowed_types`: `serde_json::Value` and `serde_json::Map`
- `disallowed_macros`: `json!`
- `disallowed_methods`: `serde_json::from_value`, `serde_json::to_value`, and the `Value`
  accessors (`get`, `get_mut`, `pointer`, `pointer_mut`, `take`, `as_*`, `is_*`)

It checks only the library and binary, so tests, `xtask/`, `build.rs` and `#[cfg(test)]` code are
not covered. Clippy does not flag `Value` variants such as `serde_json::Value::Null` or indexing
like `x["key"]`, so a test in `xtask/tests/untyped_json.rs` (part of `cargo test`) fails on those
in `src/` files that carry no untyped-JSON `#![expect]` or `#![allow]`. It reads source text: it
skips modules declared under `#[cfg(test)]` and everything after a file's first `#[cfg(test)]`.

`src/validate.rs` is exempt with a single `#![allow]`: it validates arbitrary JSON against the
OpenAPI and webhook schemas.

Each module that still uses untyped JSON starts with an `#![expect(..., reason = "untyped JSON;
remove when this module is typed")]` listing the lints that fire there. To clean a module, remove
its untyped JSON, then delete the attribute, or the lints it no longer needs; an expectation that
no longer fires fails the check. An `#![expect]` also covers child modules, so a clean child of an
expecting parent is not checked until the parent is clean.

Use instead:

- request bodies: a generated request model, through `ApiRequest::json_as`. Add the operation to
  `REQUEST_OPERATIONS` in `build.rs` to get one.
- responses: a generated response type (`RESPONSE_OPERATIONS` and `RESPONSE_SCHEMAS` in
  `build.rs`), sent with `ApiResponse::typed`.
- webhook payloads: the generated `webhook_types` (`WEBHOOK_TYPED_EVENTS` in `build.rs`).
- state: the domain records in `src/state/`.

## License

MIT
