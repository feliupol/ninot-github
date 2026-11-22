//! A pull request's review rollups: each pusher's latest opinionated review, and the
//! `reviewDecision` they add up to.

use std::collections::HashMap;

use super::types::{PullRequestReviewDecision, PullRequestReviewState};

/// A review as the review rules see it: author login, whether the author can push, state.
pub(super) type Opinion = (Option<String>, bool, PullRequestReviewState);

/// The indices, in submission order, of each pusher's latest approving, changes-requesting or
/// dismissed review: `PullRequest.latestOpinionatedReviews`.
///
/// Only reviewers with push access count, whatever `writersOnly` says: reviews from
/// non-pushers are absent with the default `writersOnly: false`. Logins compare
/// case-insensitively.
///
/// Source: `PullRequest.latestOpinionatedReviews` ("A list of latest reviews per user
/// associated with the pull request") and `PullRequestReview.authorCanPushToRepository` in
/// `spec/schema.docs.graphql`.
/// Source: observed on github.com 2026-11-15:
/// <https://github.com/octocat/Spoon-Knife/pull/4008> has approvals and a changes request, all
/// from reviewers who cannot push, an empty `latestOpinionatedReviews` and a null decision.
pub(super) fn latest_opinions(reviews: &[Opinion]) -> Vec<usize> {
    let mut latest: HashMap<String, usize> = HashMap::new();
    for (index, (author, can_push, state)) in reviews.iter().enumerate() {
        if let Some(author) = author
            && *can_push
            && matches!(
                state,
                PullRequestReviewState::Approved
                    | PullRequestReviewState::ChangesRequested
                    | PullRequestReviewState::Dismissed
            )
        {
            latest.insert(author.to_lowercase(), index);
        }
    }
    let mut indices: Vec<usize> = latest.into_values().collect();
    indices.sort_unstable();
    indices
}

/// A pull request's `reviewDecision` from its reviews when none is seeded: any changes request
/// among the latest opinionated reviews wins, then any approval, else null. A dismissed review
/// is no opinion.
///
/// Known gap: GitHub answers `REVIEW_REQUIRED` when branch protection requires more approving
/// reviews than were given. The fake models no required review count, so this never derives it.
///
/// Source: `PullRequest.reviewDecision` in `spec/schema.docs.graphql`.
/// Source: observed on github.com 2026-11-15:
/// <https://github.com/octocat/Spoon-Knife/pull/4008> reads null with only non-pusher reviews;
/// <https://github.com/cli/cli/pull/15009> reads `APPROVED` after one reviewer went from
/// changes requested to approved;
/// <https://github.com/cli/cli/pull/8361> reads `REVIEW_REQUIRED` after the same sequence,
/// because protection required more approvals.
pub(super) fn review_decision(reviews: &[Opinion]) -> Option<PullRequestReviewDecision> {
    let latest: Vec<PullRequestReviewState> = latest_opinions(reviews)
        .into_iter()
        .map(|index| reviews[index].2)
        .collect();
    if latest.contains(&PullRequestReviewState::ChangesRequested) {
        Some(PullRequestReviewDecision::ChangesRequested)
    } else if latest.contains(&PullRequestReviewState::Approved) {
        Some(PullRequestReviewDecision::Approved)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
