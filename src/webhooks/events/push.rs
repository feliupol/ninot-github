//! The `push` event, sent once per ref an accepted push changed.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Mutex;

use serde_json::Value;

use crate::api::{ApiRequest, Caller};
use crate::render;
use crate::state::{Permission, State, lock};
use crate::webhook_types;
use crate::webhooks::enqueue;
use ninot_common::git;

/// Sha GitHub reports for a ref that does not exist before or after a push.
pub const ZERO_SHA: &str = "0000000000000000000000000000000000000000";

/// Commits a push payload includes at most: "The array includes a maximum of 2048 commits"
/// (<https://docs.github.com/en/webhooks/webhook-events-and-payloads#push>).
const MAX_PUSH_COMMITS: usize = 2048;

/// "Events will not be created if more than 5000 branches are pushed at once" (same page).
const MAX_BRANCHES_PER_PUSH: usize = 5000;

/// "Events will not be created for tags when more than three tags are pushed at once" (same
/// page).
const MAX_TAGS_PER_PUSH: usize = 3;

/// Queues one `push` event per ref a push changed, sent by the caller of `request`. Reads the
/// pushed commits with the lock released so a large push does not stall other requests, and
/// not at all when no active hook of the repository wants `push`.
pub fn enqueue_push(
    state: &Mutex<State>,
    request: &ApiRequest,
    owner: &str,
    name: &str,
    git_dir: &Path,
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) {
    let Some(pusher) = request.caller.as_ref() else {
        return;
    };
    let Some((repository, sender)) = push_context(&lock(state), request, owner, name, pusher)
    else {
        return;
    };
    let html = format!("{}/{owner}/{name}", request.web_url);
    let changed = changed_refs(before, after);
    let payloads: Vec<Value> = changed
        .iter()
        .map(|(reference, old, new)| {
            let commits = if *new == ZERO_SHA {
                Vec::new()
            } else {
                let excluded: Vec<&str> = before.values().map(String::as_str).collect();
                push_commits(git_dir, &html, new, &excluded)
            };
            let payload = webhook_types::PushEvent {
                after: (*new).to_owned(),
                base_ref: None,
                before: (*old).to_owned(),
                compare: format!("{html}/compare/{}...{}", &old[..12], &new[..12]),
                created: *old == ZERO_SHA,
                deleted: *new == ZERO_SHA,
                forced: false,
                head_commit: commits.last().cloned(),
                commits,
                installation: None,
                organization: None,
                pusher: webhook_types::Committer {
                    date: None,
                    email: None,
                    name: pusher.login.clone(),
                    username: None,
                },
                ref_: (*reference).to_owned(),
                repository: repository.clone(),
                sender: sender.clone(),
            };
            serde_json::to_value(payload).expect("payloads serialize to JSON")
        })
        .collect();
    let mut state = lock(state);
    for payload in payloads {
        enqueue(&mut state, owner, name, "push", &payload, None);
    }
}

/// The repository and sender of `owner/name`'s push payloads, or `None` when no active hook of
/// the repository wants `push` or the pusher has no account.
fn push_context(
    state: &State,
    request: &ApiRequest,
    owner: &str,
    name: &str,
    pusher: &Caller,
) -> Option<(webhook_types::Repository, webhook_types::User)> {
    let account = state.account(owner)?;
    let repo = account.repo(name)?;
    if !repo.hooks.iter().any(|hook| hook.wants("push")) {
        return None;
    }
    let sender = render::simple_user(request, state.account(&pusher.login)?).into();
    let repository = super::repository(&render::full_repository(
        request,
        account,
        repo,
        Permission::default(),
    ));
    Some((repository, sender))
}

/// The refs a push changed, as `(ref, before, after)`, without those GitHub sends no event
/// for: every ref when too many branches were pushed, and tags when too many tags were.
fn changed_refs<'a>(
    before: &'a BTreeMap<String, String>,
    after: &'a BTreeMap<String, String>,
) -> Vec<(&'a str, &'a str, &'a str)> {
    let names: std::collections::BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    let changed: Vec<(&str, &str, &str)> = names
        .into_iter()
        .map(|reference| {
            let old = before.get(reference).map_or(ZERO_SHA, String::as_str);
            let new = after.get(reference).map_or(ZERO_SHA, String::as_str);
            (reference.as_str(), old, new)
        })
        .filter(|(_, old, new)| old != new)
        .collect();
    let count = |prefix: &str| {
        changed
            .iter()
            .filter(|(reference, _, _)| reference.starts_with(prefix))
            .count()
    };
    if count("refs/heads/") > MAX_BRANCHES_PER_PUSH {
        return Vec::new();
    }
    let skip_tags = count("refs/tags/") > MAX_TAGS_PER_PUSH;
    changed
        .into_iter()
        .filter(|(reference, _, _)| !(skip_tags && reference.starts_with("refs/tags/")))
        .collect()
}

/// Commits reachable from `new` but from none of `excluded`, oldest first, as payload objects.
fn push_commits(
    git_dir: &Path,
    html: &str,
    new: &str,
    excluded: &[&str],
) -> Vec<webhook_types::Commit> {
    let Ok(shas) = git::new_commits(git_dir, new, excluded) else {
        return Vec::new();
    };
    let skip = shas.len().saturating_sub(MAX_PUSH_COMMITS);
    shas[skip..]
        .iter()
        .filter_map(|sha| git::commit(git_dir, sha).ok())
        .map(|commit| {
            let (added, removed, modified) =
                git::changed_paths(git_dir, &commit.sha).unwrap_or_default();
            let person = |signature: &git::Signature| webhook_types::Committer {
                date: None,
                email: Some(signature.email.clone()),
                name: signature.name.clone(),
                username: None,
            };
            webhook_types::Commit {
                added,
                author: person(&commit.author),
                committer: person(&commit.committer),
                distinct: true,
                id: commit.sha.clone(),
                message: commit.message,
                modified,
                removed,
                timestamp: render::timestamp(&commit.committer.date),
                tree_id: commit.tree,
                url: format!("{html}/commit/{}", commit.sha),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn refs(names: &[&str], sha: &str) -> BTreeMap<String, String> {
        names
            .iter()
            .map(|name| ((*name).to_owned(), sha.to_owned()))
            .collect()
    }

    #[test]
    fn up_to_three_tags_get_events() {
        let after = refs(
            &[
                "refs/heads/main",
                "refs/tags/a",
                "refs/tags/b",
                "refs/tags/c",
            ],
            "1",
        );
        assert_eq!(changed_refs(&BTreeMap::new(), &after).len(), 4);
    }

    #[test]
    fn more_than_three_tags_get_no_tag_events() {
        let after = refs(
            &[
                "refs/heads/main",
                "refs/tags/a",
                "refs/tags/b",
                "refs/tags/c",
                "refs/tags/d",
            ],
            "1",
        );
        let before = BTreeMap::new();
        let changed = changed_refs(&before, &after);
        assert_eq!(changed, [("refs/heads/main", ZERO_SHA, "1")]);
    }

    #[test]
    fn more_than_5000_branches_get_no_events() {
        let names: Vec<String> = (0..=MAX_BRANCHES_PER_PUSH)
            .map(|index| format!("refs/heads/b{index}"))
            .collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        assert!(changed_refs(&BTreeMap::new(), &refs(&names, "1")).is_empty());
    }

    fn hook(events: &[&str], active: bool) -> crate::state::Hook {
        crate::state::Hook {
            id: 1,
            active,
            events: events.iter().map(|event| (*event).to_owned()).collect(),
            url: "http://127.0.0.1:1/".to_owned(),
            content_type: "json".to_owned(),
            secret: None,
            insecure_ssl: "0".to_owned(),
            created_at: crate::state::now(),
            updated_at: crate::state::now(),
        }
    }

    #[test]
    fn push_payload_is_built_only_for_repositories_with_push_hooks() {
        let mut state = State::default();
        state.add_user("alice");
        state.create_repo("alice", "tool", false, None).unwrap();
        let alice = Caller {
            login: "alice".to_owned(),
            scopes: Vec::new(),
        };
        let request = ApiRequest::for_urls("http://localhost", Some(alice.clone()));
        let context = |state: &State| push_context(state, &request, "alice", "tool", &alice);
        assert!(context(&state).is_none());
        let repo = state
            .account_mut("alice")
            .unwrap()
            .repo_mut("tool")
            .unwrap();
        repo.hooks = vec![hook(&["issues"], true), hook(&["push"], false)];
        assert!(context(&state).is_none());
        let repo = state
            .account_mut("alice")
            .unwrap()
            .repo_mut("tool")
            .unwrap();
        repo.hooks.push(hook(&["*"], true));
        assert!(context(&state).is_some());
    }

    #[test]
    fn unchanged_refs_are_ignored() {
        let before = refs(&["refs/heads/main"], "1");
        assert!(changed_refs(&before, &before).is_empty());
    }
}
