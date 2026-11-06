# ninot-github

A stateful, in-memory fake of the GitHub REST API and git hosting, for
end-to-end tests that must not touch the network or a real account.

Unlike a mock, it keeps state: repositories you create can be cloned and
pushed to, and what you push is visible through the API. It can also
misbehave on purpose, adding latency and faults, so clients are tested
against the failures real networks produce.

Its API surface is generated from GitHub's official OpenAPI description.
Operations without an implementation answer `501 Not Implemented` with a message
naming their `operationId`.
GraphQL serves GitHub's published schema; fields without an implementation
answer the error `Not implemented by ninot-github: Type.field`.

## Use

In Rust tests, add `ninot-github` to `[dev-dependencies]`:

```rust
let server = ninot_github::Server::start()?;
server.add_user("alice");
let token = server.add_token("alice", &["repo"]);
// REST at server.url() and server.url() + "/api/v3", git at server.url()/{owner}/{repo}.git
```

From anything else, run `ninot-github serve [--port PORT] [--tls]`, which prints the URL and
an admin token.

With `--tls` (or `Options { tls: true, .. }`), the GitHub CLI works against it:

```sh
GH_HOST=127.0.0.1:PORT GH_ENTERPRISE_TOKEN=TOKEN SSL_CERT_FILE=CA GIT_SSL_CAINFO=CA gh repo list
```

Pull-request history (the issue timeline, pull-request commits and files) is derived from
the same state the other endpoints serve; see [python/README.md](python/README.md) for what
it covers and how fixtures seed it.

Faults are installed with `Server::add_fault`.

## Actions runner

The official `actions/runner` binary can register with the fake (`config.sh --url
https://HOST/ORG --token ...`, with a token from `POST /orgs/{org}/actions/runners/registration-token`)
and run jobs. `Repository::queue_script_job(labels, scripts)` queues a job whose steps are
`run:` scripts; a registered runner of the repository's organization that carries every label
takes it, and the job and its workflow run then move to `in_progress` and `completed`, with the
runner's step outcomes and conclusion, through the usual REST endpoints. An ephemeral runner is
removed after its job.

The fake serves what the runner calls under `/_services/`: the OAuth token exchange, the
broker session and message queue, and the run service (`acquirejob`, `renewjob`,
`completejob`). The assertion's signature is not verified. Not served: the results service, so
step logs and summaries are not uploaded; actions to download; containers and services; cancellation.
The runner drops the port of its `--url`, so the fake must listen on 443.

`tests/runner_e2e.rs` runs the real runner (pinned release, sha256-checked, cached) against the
fake; see its header for how to run it (`NINOT_RUNNER_E2E=1`).

GitHub's rate limits apply by default; change them with `Options { rate_limits, .. }` and
expire windows with `Server::advance_clock`.

## Status

Early development.

## Build and test

Rust: `cargo fmt --all -- --check`,
`cargo clippy --all-targets --workspace -- -D warnings`,
`cargo test --all-targets --workspace --exclude ninot-github-python`.

Python package: `maturin develop -m python/Cargo.toml`, then `pytest -q python/tests`.

## License

MIT
