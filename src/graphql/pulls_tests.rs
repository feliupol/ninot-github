//! Unit tests of the pull request review rollups and stored-commit lookup.

use super::*;

#[test]
fn stored_commit_falls_back_only_without_storage_or_commit() {
    let dir =
        std::env::temp_dir().join(format!("ninot-github-stored-commit-{}", std::process::id()));
    let git_dir = dir.join("repo.git");
    std::fs::create_dir_all(&git_dir).unwrap();
    git::init_bare(&git_dir, "main").unwrap();
    git::commit_readme(&git_dir, "main", "repo", &crate::handlers::BOT).unwrap();
    let sha = git::resolve_commit(&git_dir, "refs/heads/main")
        .unwrap()
        .unwrap();

    assert!(stored_commit(None, &sha).unwrap().is_none());
    let missing = "0000000000000000000000000000000000000000";
    assert!(stored_commit(Some(&git_dir), missing).unwrap().is_none());
    assert_eq!(
        stored_commit(Some(&git_dir), &sha).unwrap().unwrap().sha,
        sha
    );

    let object = git_dir.join("objects").join(&sha[..2]).join(&sha[2..]);
    let mut permissions = std::fs::metadata(&object).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(&object, permissions).unwrap();
    std::fs::write(&object, b"not a zlib stream").unwrap();
    let Err(error) = stored_commit(Some(&git_dir), &sha) else {
        panic!("a corrupt commit object reads as an error");
    };
    assert!(error.message.contains(&sha), "{}", error.message);
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn review_decision_takes_each_pushers_latest_opinion() {
    use PullRequestReviewDecision::{Approved, ChangesRequested};
    use PullRequestReviewState::{
        Approved as Approve, ChangesRequested as Request, Commented as Comment,
        Dismissed as Dismiss,
    };

    let pusher = |login: &str, state| (Some(login.to_owned()), true, state);
    let reader = |login: &str, state| (Some(login.to_owned()), false, state);

    // No reviews, or only comments: null.
    assert_eq!(review_decision(&[]), None);
    assert_eq!(review_decision(&[pusher("alice", Comment)]), None);
    // Reviewers without push access never count.
    assert_eq!(review_decision(&[reader("carol", Approve)]), None);
    assert_eq!(
        review_decision(&[reader("carol", Request), pusher("alice", Approve)]),
        Some(Approved)
    );
    assert_eq!(review_decision(&[(None, false, Approve)]), None);
    // Each pusher's latest opinion counts.
    assert_eq!(
        review_decision(&[pusher("alice", Request), pusher("alice", Approve)]),
        Some(Approved)
    );
    assert_eq!(
        review_decision(&[pusher("alice", Approve), pusher("alice", Request)]),
        Some(ChangesRequested)
    );
    assert_eq!(
        review_decision(&[pusher("alice", Approve), pusher("alice", Comment)]),
        Some(Approved)
    );
    assert_eq!(
        review_decision(&[pusher("Alice", Request), pusher("alice", Approve)]),
        Some(Approved)
    );
    // Any pusher's changes request wins over other pushers' approvals.
    assert_eq!(
        review_decision(&[pusher("alice", Approve), pusher("bob", Request)]),
        Some(ChangesRequested)
    );
    // A dismissed latest review is no opinion.
    assert_eq!(
        review_decision(&[pusher("alice", Request), pusher("alice", Dismiss)]),
        None
    );
    assert_eq!(
        review_decision(&[pusher("alice", Dismiss), pusher("alice", Approve)]),
        Some(Approved)
    );
}

#[test]
fn latest_opinions_keep_each_pushers_latest_opinionated_review() {
    use PullRequestReviewState::{
        Approved as Approve, ChangesRequested as Request, Commented as Comment,
        Dismissed as Dismiss,
    };

    let pusher = |login: &str, state| (Some(login.to_owned()), true, state);
    let reader = |login: &str, state| (Some(login.to_owned()), false, state);

    assert_eq!(latest_opinions(&[]), Vec::<usize>::new());
    assert_eq!(
        latest_opinions(&[reader("carol", Approve), (None, false, Request)]),
        Vec::<usize>::new()
    );
    assert_eq!(
        latest_opinions(&[
            pusher("alice", Request),
            pusher("bob", Approve),
            pusher("Alice", Approve),
            pusher("alice", Comment),
            reader("carol", Request),
            pusher("bob", Dismiss),
        ]),
        vec![2, 5]
    );
}
