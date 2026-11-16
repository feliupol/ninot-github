//! Reactions on issues, pull requests, comments and releases.
//!
//! Held outside repositories, keyed by repository id, so renames and transfers keep them.

use std::collections::BTreeMap;

use chrono::{DateTime, Utc};

use super::Name;

/// What a reaction is on. Pull requests are issues here, as `/issues/{number}/reactions` serves
/// both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReactionSubject {
    /// Issue or pull request `number` of repository `repo`.
    Issue { repo: u64, number: u64 },
    /// Conversation comment `id` of an issue or pull request.
    IssueComment { repo: u64, id: u64 },
    /// Pull request review comment `id`.
    ReviewComment { repo: u64, id: u64 },
    /// Release `id`.
    Release { repo: u64, id: u64 },
}

/// The emoji of a reaction.
/// Source: <https://docs.github.com/en/rest/reactions/reactions#about-reactions>
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReactionContent {
    PlusOne,
    MinusOne,
    Laugh,
    Confused,
    Heart,
    Hooray,
    Rocket,
    Eyes,
}

impl ReactionContent {
    /// Every content, in the order GitHub's rollup lists them.
    pub const ALL: [Self; 8] = [
        Self::PlusOne,
        Self::MinusOne,
        Self::Laugh,
        Self::Confused,
        Self::Heart,
        Self::Hooray,
        Self::Rocket,
        Self::Eyes,
    ];

    /// The REST `content` value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PlusOne => "+1",
            Self::MinusOne => "-1",
            Self::Laugh => "laugh",
            Self::Confused => "confused",
            Self::Heart => "heart",
            Self::Hooray => "hooray",
            Self::Rocket => "rocket",
            Self::Eyes => "eyes",
        }
    }

    /// The content named by a REST `content` value.
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|content| content.as_str() == value)
    }
}

/// One user's reaction.
#[derive(Clone, Debug)]
pub struct Reaction {
    /// Reaction id, unique across the server.
    pub id: u64,
    /// Login of the user who reacted.
    pub user: Name,
    pub content: ReactionContent,
    pub created_at: DateTime<Utc>,
}

/// Every reaction, by subject, oldest first.
#[derive(Default)]
pub struct Reactions(BTreeMap<ReactionSubject, Vec<Reaction>>);

impl Reactions {
    /// The reactions on `subject`, oldest first.
    pub fn on(&self, subject: ReactionSubject) -> &[Reaction] {
        self.0.get(&subject).map_or(&[], Vec::as_slice)
    }

    /// The reaction of `user` with `content` on `subject`, if any.
    pub fn find(
        &self,
        subject: ReactionSubject,
        user: &str,
        content: ReactionContent,
    ) -> Option<&Reaction> {
        self.on(subject)
            .iter()
            .find(|reaction| reaction.content == content && reaction.user.matches(user))
    }

    /// Adds `reaction` to `subject`.
    pub fn add(&mut self, subject: ReactionSubject, reaction: Reaction) {
        self.0.entry(subject).or_default().push(reaction);
    }

    /// Removes reaction `id` from `subject`, returning it.
    pub fn remove(&mut self, subject: ReactionSubject, id: u64) -> Option<Reaction> {
        let reactions = self.0.get_mut(&subject)?;
        let index = reactions.iter().position(|reaction| reaction.id == id)?;
        Some(reactions.remove(index))
    }

    /// How many reactions of each content `subject` has, as rollups count them, with the total.
    pub fn counts(&self, subject: ReactionSubject) -> ([i64; 8], i64) {
        let mut counts = [0; 8];
        for reaction in self.on(subject) {
            let index = ReactionContent::ALL
                .iter()
                .position(|content| *content == reaction.content)
                .expect("ALL lists every content");
            counts[index] += 1;
        }
        (counts, counts.iter().sum())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contents_round_trip_through_their_rest_values() {
        for content in ReactionContent::ALL {
            assert_eq!(ReactionContent::parse(content.as_str()), Some(content));
        }
        assert_eq!(ReactionContent::parse("thumbsup"), None);
    }

    #[test]
    fn reactions_are_kept_per_subject_and_counted() {
        let issue = ReactionSubject::Issue { repo: 1, number: 2 };
        let other = ReactionSubject::Release { repo: 1, id: 2 };
        let mut reactions = Reactions::default();
        for (id, user, content) in [
            (10, "alice", ReactionContent::Heart),
            (11, "Bob", ReactionContent::Heart),
            (12, "alice", ReactionContent::PlusOne),
        ] {
            reactions.add(
                issue,
                Reaction {
                    id,
                    user: Name::new(user),
                    content,
                    created_at: DateTime::UNIX_EPOCH,
                },
            );
        }
        assert!(reactions.on(other).is_empty());
        assert_eq!(
            reactions
                .find(issue, "bob", ReactionContent::Heart)
                .map(|r| r.id),
            Some(11)
        );
        assert_eq!(reactions.counts(issue), ([1, 0, 0, 0, 2, 0, 0, 0], 3));
        assert_eq!(reactions.remove(other, 11).map(|r| r.id), None);
        assert_eq!(reactions.remove(issue, 11).map(|r| r.id), Some(11));
        assert_eq!(reactions.counts(issue).1, 2);
    }
}
