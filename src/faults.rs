//! Deliberate misbehaviour: latency, error statuses, dropped and truncated responses.
//!
//! The rules and the applied faults come from [`ninot_common::faults`]; the targets and
//! faults below are the ones GitHub's API and git hosting can misbehave in.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// A fault rule over GitHub's targets and faults.
pub type Rule = ninot_common::faults::Rule<Target, Fault>;

/// A fault that was applied, recorded for inspection.
pub type Applied = ninot_common::faults::Applied<Fault>;

/// What a fault rule applies to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Target {
    /// Every request.
    Any,
    /// An API operation, by `operationId`, such as `repos/get`.
    Operation(String),
    /// A git service: `upload-pack` (clone and fetch) or `receive-pack` (push).
    Git(String),
    /// Requests whose path starts with this prefix, after any `/api/v3` prefix.
    PathPrefix(String),
}

/// What happens to a request a rule hits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Fault {
    /// Waits this many milliseconds, then serves the request normally.
    LatencyMs(u64),
    /// Answers with this status and a GitHub-shaped error body instead of serving the request.
    Status(u16),
    /// GitHub's secondary rate limit: `403` with `retry-after`.
    SecondaryRateLimit,
    /// Closes the connection without sending a response.
    Drop,
    /// Serves the request but closes the connection halfway through the response body.
    Truncate,
}

impl ninot_common::faults::Fault for Fault {
    fn latency(&self) -> Option<Duration> {
        match self {
            Self::LatencyMs(ms) => Some(Duration::from_millis(*ms)),
            _ => None,
        }
    }

    fn transport(&self) -> ninot_common::faults::Transport {
        match self {
            Self::Drop => ninot_common::faults::Transport::Drop,
            Self::Truncate => ninot_common::faults::Transport::Truncate,
            _ => ninot_common::faults::Transport::Answer,
        }
    }
}

/// The rules of one fake, over GitHub's targets and faults.
pub(crate) type Faults = ninot_common::faults::Faults<Target, Fault>;

/// What a request is, for matching rules.
pub(crate) struct RequestInfo<'a> {
    pub method: &'a str,
    pub path: &'a str,
    pub operation: Option<&'a str>,
    pub git_service: Option<&'a str>,
}

impl Target {
    pub(crate) fn matches(&self, request: &RequestInfo) -> bool {
        match self {
            Self::Any => true,
            Self::Operation(id) => request.operation == Some(id.as_str()),
            Self::Git(service) => request.git_service == Some(service.as_str()),
            Self::PathPrefix(prefix) => request.path.starts_with(prefix.as_str()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn git_and_operation_targets_match_their_own_kind_of_request() {
        let clone = RequestInfo {
            method: "GET",
            path: "/admin/tool/info/refs",
            operation: None,
            git_service: Some("upload-pack"),
        };
        assert!(Target::Any.matches(&clone));
        assert!(Target::Git("upload-pack".into()).matches(&clone));
        assert!(!Target::Git("receive-pack".into()).matches(&clone));
        assert!(!Target::Operation("repos/get".into()).matches(&clone));
        assert!(!Target::PathPrefix("/repos/".into()).matches(&clone));
        let get = RequestInfo {
            method: "GET",
            path: "/repos/admin/tool",
            operation: Some("repos/get"),
            git_service: None,
        };
        assert!(Target::Operation("repos/get".into()).matches(&get));
        assert!(Target::PathPrefix("/repos/".into()).matches(&get));
    }
}
