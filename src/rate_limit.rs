//! GitHub's primary and secondary rate limits, on a clock tests can advance.

use std::collections::HashMap;
use std::net::IpAddr;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// A bucket of the primary limit, as named in `X-RateLimit-Resource`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Resource {
    Core,
    Search,
    Graphql,
}

impl Resource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Core => "core",
            Self::Search => "search",
            Self::Graphql => "graphql",
        }
    }
}

/// Requests allowed per window of one resource.
#[derive(Clone, Copy, Debug)]
pub struct Quota {
    pub authenticated: u32,
    pub anonymous: u32,
    pub window: Duration,
}

/// Every limit the fake enforces; the default is GitHub's.
#[derive(Clone, Debug)]
pub struct Limits {
    pub core: Quota,
    pub search: Quota,
    pub graphql: Quota,
    /// Requests one client may have in flight at once.
    pub concurrent: u32,
    /// `POST`, `PATCH`, `PUT` and `DELETE` REST requests one client may make per minute.
    pub content_creation_per_minute: u32,
}

impl Default for Limits {
    fn default() -> Self {
        let hour = Duration::from_secs(3600);
        let minute = Duration::from_secs(60);
        Self {
            core: Quota {
                authenticated: 5000,
                anonymous: 60,
                window: hour,
            },
            search: Quota {
                authenticated: 30,
                anonymous: 10,
                window: minute,
            },
            graphql: Quota {
                authenticated: 5000,
                anonymous: 0,
                window: hour,
            },
            concurrent: 100,
            content_creation_per_minute: 80,
        }
    }
}

/// Who a limit is counted against: a valid token, or the address of an anonymous caller.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Client {
    Token(String),
    Ip(IpAddr),
}

/// One client's window of one resource.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Window {
    pub limit: u32,
    pub used: u32,
    /// Unix time in seconds at which the window restarts.
    pub reset: u64,
}

impl Window {
    pub fn remaining(&self) -> u32 {
        self.limit.saturating_sub(self.used)
    }

    /// `X-RateLimit-*` headers describing this window.
    pub fn headers(&self, resource: Resource) -> Vec<(String, String)> {
        [
            ("x-ratelimit-limit", self.limit.to_string()),
            ("x-ratelimit-remaining", self.remaining().to_string()),
            ("x-ratelimit-used", self.used.to_string()),
            ("x-ratelimit-reset", self.reset.to_string()),
            ("x-ratelimit-resource", resource.as_str().to_owned()),
        ]
        .into_iter()
        .map(|(name, value)| (name.to_owned(), value))
        .collect()
    }
}

/// Why a request was refused, with the client's window of the requested resource.
#[derive(Debug, PartialEq)]
pub(crate) enum Refusal {
    Primary(Window),
    Secondary { window: Window, retry_after: u64 },
}

/// Windows and in-flight requests of every client.
#[derive(Default)]
pub(crate) struct RateLimits {
    pub limits: Limits,
    clock_offset: Duration,
    windows: HashMap<(Client, Resource), Window>,
    content_creation: HashMap<Client, Window>,
    in_flight: HashMap<Client, u32>,
}

impl RateLimits {
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            ..Self::default()
        }
    }

    /// Unix time in seconds on the test clock.
    pub fn now(&self) -> u64 {
        (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            + self.clock_offset)
            .as_secs()
    }

    /// Moves the test clock forward.
    pub fn advance(&mut self, by: Duration) {
        self.clock_offset += by;
    }

    /// The client's current window of `resource`, restarting it when it has expired.
    pub fn window(&mut self, client: &Client, resource: Resource) -> Window {
        let quota = match resource {
            Resource::Core => self.limits.core,
            Resource::Search => self.limits.search,
            Resource::Graphql => self.limits.graphql,
        };
        let limit = match client {
            Client::Token(_) => quota.authenticated,
            Client::Ip(_) => quota.anonymous,
        };
        let now = self.now();
        let window = self
            .windows
            .entry((client.clone(), resource))
            .or_insert(Window {
                limit,
                used: 0,
                reset: 0,
            });
        if now >= window.reset {
            *window = Window {
                limit,
                used: 0,
                reset: now + quota.window.as_secs(),
            };
        }
        *window
    }

    /// Counts a request against the client's window, or refuses it. An admitted request must
    /// be [released](Self::release) once answered.
    pub fn admit(
        &mut self,
        client: &Client,
        resource: Resource,
        creates_content: bool,
    ) -> Result<Window, Refusal> {
        let window = self.window(client, resource);
        if window.remaining() == 0 {
            return Err(Refusal::Primary(window));
        }
        if self.in_flight.get(client).copied().unwrap_or(0) >= self.limits.concurrent {
            return Err(Refusal::Secondary {
                window,
                retry_after: 60,
            });
        }
        if creates_content {
            let now = self.now();
            let limit = self.limits.content_creation_per_minute;
            let created = self
                .content_creation
                .entry(client.clone())
                .or_insert(Window {
                    limit,
                    used: 0,
                    reset: 0,
                });
            if now >= created.reset {
                *created = Window {
                    limit,
                    used: 0,
                    reset: now + 60,
                };
            }
            if created.remaining() == 0 {
                return Err(Refusal::Secondary {
                    window,
                    retry_after: created.reset - now,
                });
            }
            created.used += 1;
        }
        *self.in_flight.entry(client.clone()).or_default() += 1;
        let window = self.windows.get_mut(&(client.clone(), resource)).unwrap();
        window.used += 1;
        Ok(*window)
    }

    /// Ends a request [admitted](Self::admit) earlier.
    pub fn release(&mut self, client: &Client) {
        if let Some(count) = self.in_flight.get_mut(client) {
            *count -= 1;
        }
    }
}
