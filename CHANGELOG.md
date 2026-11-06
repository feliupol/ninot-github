# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and versions follow the calendar: `<year>.<ISO week>.<n>`.

## [2026.45.0] - 2026-11-05

### Added

- Stateful, in-memory fake of the GitHub REST API and git hosting for end-to-end tests that must not touch the network or a real account.
- REST routing, authentication and GitHub-style errors, generated from the pinned GitHub OpenAPI description; operations without an implementation answer `501 Not Implemented` with their `operationId`.
- Accounts, organizations, repositories, repository settings, labels and webhook events.
- Git hosting over smart HTTP; refs, commits, branches and contents (including creating and updating files) through the REST API; gitlinks served as submodules.
- PRs with reviews, requested reviewers, edits, retargeting, closing and reopening, and timeline, commits and files with paging.
- Releases and release assets.
- Signed webhook delivery, with payloads generated from octokit's webhook schemas.
- Self-hosted runners: the official Actions runner registers, takes a queued job (`Repository::queue_script_job`), runs its `run:` steps and reports the result.
- Search, including `@me` and team review requests.
- GraphQL served from GitHub's published schema, including projects and auto-merge; unimplemented fields answer `Not implemented by ninot-github: Type.field`.
- GitHub Actions run, job and secret state.
- Typed resource fixtures and typed models for REST and GraphQL payloads.
- Latency and fault injection, installed with `Server::add_fault` or `POST /_ninot/faults`, and GitHub's rate limits, configurable through `Options` and advanced with `Server::advance_clock`.
- Validation of request bodies, responses and webhook payloads against the spec.
- HTTPS with `--tls`, so the `gh` command-line client works against the server.
- `ninot-github serve [--port PORT] [--tls]` command-line server that prints its URL and an admin token.
- Python bindings and a pytest fixture.
- Requires Rust 1.88 or newer (edition 2024).
