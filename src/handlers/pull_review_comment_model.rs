//! The review comment model: where it attaches and how it renders.

use chrono::{DateTime, Utc};
use serde_json::{Map, Value};

use super::repos::Found;
use crate::api::{ApiRequest, ApiResponse};
use crate::state;
use crate::types;

/// Where a review comment attaches in the diff.
#[derive(Default)]
pub(super) struct Anchor {
    pub path: String,
    pub line: Option<i64>,
    pub position: Option<i64>,
    pub side: Option<String>,
    pub start_line: Option<i64>,
    pub start_side: Option<String>,
    pub file: bool,
}

impl Anchor {
    /// Reads the anchor fields of a review comment request body.
    pub(super) fn from_map(body: &Map<String, Value>) -> Result<Self, ApiResponse> {
        let text = |name: &str| match body.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(Value::String(text)) => Ok(Some(text.clone())),
            Some(_) => Err(invalid(name)),
        };
        let number = |name: &str| match body.get(name) {
            None | Some(Value::Null) => Ok(None),
            Some(value) => value.as_i64().map(Some).ok_or_else(|| invalid(name)),
        };
        let path = text("path")?.filter(|path| !path.is_empty());
        let file = match text("subject_type")?.as_deref() {
            None | Some("line") => false,
            Some("file") => true,
            Some(_) => return Err(invalid("subject_type")),
        };
        let anchor = Self {
            path: path.ok_or_else(|| missing("path"))?,
            line: number("line")?,
            position: number("position")?,
            side: text("side")?,
            start_line: number("start_line")?,
            start_side: text("start_side")?,
            file,
        };
        anchor.checked()
    }

    /// Reads the anchor of one entry of a review's `comments` list.
    pub(super) fn from_item(
        item: &types::PullsCreateReviewRequestCommentsItem,
    ) -> Result<Self, ApiResponse> {
        Self {
            path: item.path.clone(),
            line: item.line,
            position: item.position,
            side: item.side.clone(),
            start_line: item.start_line,
            start_side: item.start_side.clone(),
            file: false,
        }
        .checked()
    }

    fn checked(self) -> Result<Self, ApiResponse> {
        for (name, side) in [("side", &self.side), ("start_side", &self.start_side)] {
            if side
                .as_deref()
                .is_some_and(|side| side != "LEFT" && side != "RIGHT")
            {
                return Err(invalid(name));
            }
        }
        if self.path.is_empty() {
            return Err(missing("path"));
        }
        if !self.file && self.line.is_none() && self.position.is_none() {
            return Err(missing("line"));
        }
        if self
            .start_line
            .is_some_and(|start| self.line.is_some_and(|line| start > line) || self.line.is_none())
        {
            return Err(invalid("start_line"));
        }
        Ok(self)
    }
}

pub(super) fn invalid(field: &str) -> ApiResponse {
    ApiResponse::validation_failed("PullRequestReviewComment", field, "invalid")
}

pub(super) fn missing(field: &str) -> ApiResponse {
    ApiResponse::validation_failed("PullRequestReviewComment", field, "missing_field")
}

fn side(name: Option<&str>) -> types::PullRequestReviewCommentSide {
    if name == Some("LEFT") {
        types::PullRequestReviewCommentSide::Left
    } else {
        types::PullRequestReviewCommentSide::Right
    }
}

/// What a new review comment says and where it lives.
pub(super) struct NewComment<'a> {
    pub id: u64,
    pub body: String,
    pub anchor: Anchor,
    pub commit_id: &'a str,
    pub review_id: Option<i64>,
    pub in_reply_to: Option<i64>,
    pub diff_hunk: String,
}

/// Renders a review comment by `author` on pull request `number`.
pub(super) fn build(
    request: &ApiRequest,
    found: &Found,
    number: u64,
    author: &types::SimpleUser,
    new: NewComment<'_>,
) -> types::PullRequestReviewComment {
    let now = render_now();
    let id = crate::render::id(new.id);
    let pull_url = format!(
        "{}/repos/{}/{}/pulls/{number}",
        request.api_url, found.owner, found.name
    );
    let url = format!(
        "{}/repos/{}/{}/pulls/comments/{id}",
        request.api_url, found.owner, found.name
    );
    let html_url = format!(
        "{}/{}/{}/pull/{number}#discussion_r{id}",
        request.web_url, found.owner, found.name
    );
    let anchor = new.anchor;
    // The diff index is not tracked; the line stands in for it, as the response models require one.
    let diff_position = anchor.position.or(anchor.line).unwrap_or(0);
    types::PullRequestReviewComment {
        author_association: types::AuthorAssociation::Owner,
        body: new.body,
        body_html: None,
        body_text: None,
        commit_id: new.commit_id.to_owned(),
        created_at: now,
        diff_hunk: new.diff_hunk,
        html_url: html_url.clone(),
        id,
        in_reply_to_id: new.in_reply_to,
        line: anchor.line,
        links: types::PullRequestReviewCommentLinks {
            html: types::PullRequestReviewCommentLinksHtml { href: html_url },
            pull_request: types::PullRequestReviewCommentLinksPullRequest {
                href: pull_url.clone(),
            },
            self_: types::PullRequestReviewCommentLinksSelf { href: url.clone() },
        },
        node_id: crate::render::node_id("PullRequestReviewComment", new.id),
        original_commit_id: new.commit_id.to_owned(),
        original_line: anchor.line,
        original_position: Some(diff_position),
        original_start_line: anchor.start_line,
        path: anchor.path,
        position: Some(diff_position),
        pull_request_review_id: new.review_id,
        pull_request_url: pull_url,
        reactions: None,
        side: side(anchor.side.as_deref()),
        start_line: anchor.start_line,
        start_side: anchor.start_side.as_deref().map(|name| {
            if name == "LEFT" {
                types::PullRequestReviewCommentStartSide::Left
            } else {
                types::PullRequestReviewCommentStartSide::Right
            }
        }),
        subject_type: Some(if anchor.file {
            types::PullRequestReviewCommentSubjectType::File
        } else {
            types::PullRequestReviewCommentSubjectType::Line
        }),
        updated_at: now,
        url,
        user: Some(author.clone().into()),
    }
}

pub(super) fn render_now() -> DateTime<Utc> {
    crate::render::timestamp(&state::now())
}
