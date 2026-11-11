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
`WorkflowJob` and `ActionsSecret`, which live in [`server`](src/server.rs).

## Git hosting

Each repository is a real git repository served over smart HTTP at
`{url}/{owner}/{repo}.git`. Authenticate with Basic auth, username `x-access-token` and the
token as the password. [`Repository::commit_file`](src/server.rs) writes a commit through
`git` on a fixture path when the test does not want to drive a work tree.

## Fault injection

[`Server::add_fault`](src/server.rs) installs a [`Rule`](src/faults.rs) of a [`Target`](src/faults.rs)
and a [`Fault`](src/faults.rs). Targets are every request, one `operationId`, a git service
(`upload-pack` or `receive-pack`), or a path prefix. Faults are a delay, an HTTP status, a
secondary rate limit, a dropped connection, or a truncated body. `clear_faults` removes the
rules, `seed_faults` restarts the random sequence, and `applied_faults` lists what fired.
See [`examples/faults.rs`](examples/faults.rs).

## Rate limits and the clock

The default limits are GitHub's: core 5000 per hour authenticated and 60 anonymous, search
30 and 10 per minute, GraphQL 5000 and 0 per hour, 100 requests in flight, and 80
content-creating REST requests per minute. Set them with
`Options { rate_limits, .. }`. A client over the concurrent or content-creation cap gets a
secondary rate limit (`403` and `retry-after`). [`Server::advance_clock`](src/server.rs)
expires windows without waiting.

## Webhooks

`POST /repos/{owner}/{repo}/hooks` creates a hook. Deliveries are POSTed to its URL and, when
the hook has a secret, signed with `X-Hub-Signature-256` (HMAC-SHA256, `sha256=` prefix).
[`Server::deliveries`](src/server.rs) returns them; `wait_for_deliveries` waits until enough
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
[`Repository::queue_script_job`](src/server.rs). Registration uses the registration-token
endpoint, and the runner's own calls are served under `/_services/`. The runner drops the
port of its `--url`, so it has to reach the fake on port 443. Details, including
`NINOT_RUNNER_E2E`, are in [docs/actions-runner.md](docs/actions-runner.md).
[`examples/orgs_and_runners.rs`](examples/orgs_and_runners.rs) creates an organization, a
team and a runner.

## Python package

[`python/`](python/) is a PyO3 binding of the same handles (`Server`, `User`, `Org`, `Team`,
`Runner`, `Repository` and the repository resources). `pip install ./python` or
`maturin develop` builds it. Usage and the pytest plugin are in
[python/README.md](python/README.md).

<!-- doc-gen completion -->

## Status

REST operations: 156 / 1231 (12.7%).

| Tag | Implemented | Total | % |
| --- | ---: | ---: | ---: |
| actions | 30 | 200 | 15.0% |
| activity | 0 | 34 | 0.0% |
| agent-tasks | 0 | 5 | 0.0% |
| agents | 0 | 30 | 0.0% |
| apps | 2 | 37 | 5.4% |
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
| meta | 2 | 5 | 40.0% |
| migrations | 0 | 22 | 0.0% |
| oidc | 0 | 8 | 0.0% |
| orgs | 1 | 116 | 0.9% |
| packages | 0 | 27 | 0.0% |
| private-registries | 0 | 6 | 0.0% |
| projects | 0 | 26 | 0.0% |
| pulls | 13 | 34 | 38.2% |
| rate-limit | 1 | 1 | 100.0% |
| reactions | 0 | 15 | 0.0% |
| repos | 36 | 203 | 17.7% |
| search | 2 | 7 | 28.6% |
| secret-scanning | 0 | 17 | 0.0% |
| security-advisories | 0 | 10 | 0.0% |
| teams | 0 | 32 | 0.0% |
| users | 2 | 47 | 4.3% |

GitHub CLI leaf commands: 18 / 205 (8.8%).

GraphQL resolvers: 25 / 5577 schema fields (0.4%). The published schema is served. A field without a resolver reads the same-named key of its parent object and returns the error `Not implemented by ninot-github: Type.field` when that key is absent.

Python bindings: 67 / 77 public methods.

| Type | Exposed | Total | Missing |
| --- | ---: | ---: | --- |
| ActionsSecret | 3 | 3 | none |
| Commit | 3 | 3 | none |
| Issue | 2 | 2 | none |
| Org | 3 | 3 | none |
| Project | 3 | 3 | none |
| PullRequest | 8 | 8 | none |
| Repository | 16 | 20 | `commit_file_by`, `create_app_installation`, `create_installation_with_secrets_permission`, `queue_script_job` |
| Review | 2 | 2 | none |
| Runner | 2 | 2 | none |
| Server | 18 | 24 | `deliveries`, `hits`, `runner`, `user`, `wait`, `wait_for_deliveries` |
| Team | 2 | 2 | none |
| User | 3 | 3 | none |
| WorkflowRun | 2 | 2 | none |

<!-- end-doc-gen -->

## Build and test

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --workspace -- -D warnings
cargo test --all-targets --workspace --exclude ninot-github-python
cargo build --examples
cargo xtask completion check
```

`cargo xtask` is the maintainer alias (`completion check|update`,
`gh-coverage report|check|update`). The Python package is tested with
`maturin develop -m python/Cargo.toml` and `pytest -q python/tests`.

## License

MIT
