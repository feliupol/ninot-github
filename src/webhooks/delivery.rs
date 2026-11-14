//! Recording, signing and sending webhook deliveries.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};
use std::sync::mpsc;
use std::sync::{Mutex, Weak};
use std::time::{Duration, Instant};

use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;

use crate::state::{Delivery, State, lock, now};
use crate::validate;

/// `User-Agent` of every delivery, shaped like GitHub's `GitHub-Hookshot/<id>`.
const USER_AGENT: &str = "GitHub-Hookshot/ninot-github";

/// The headers GitHub sends with a webhook delivery.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeliveryHeaders {
    /// `X-GitHub-Event`, such as `push`.
    pub event: String,
    /// `X-GitHub-Delivery`, a GUID unique to the delivery.
    pub delivery: String,
    /// `X-GitHub-Hook-ID`.
    pub hook_id: u64,
    /// `X-GitHub-Hook-Installation-Target-ID`: the repository's id for repository hooks.
    pub target_id: u64,
    /// `X-GitHub-Hook-Installation-Target-Type`, such as `repository`.
    pub target_type: &'static str,
    /// `Content-Type`: `application/json` or `application/x-www-form-urlencoded`.
    pub content_type: &'static str,
    /// `X-Hub-Signature-256`, present when the hook has a secret.
    pub signature: Option<String>,
}

impl DeliveryHeaders {
    /// The headers as name and value pairs, in the order they are sent.
    pub fn pairs(&self) -> Vec<(&'static str, String)> {
        let mut pairs = vec![
            ("User-Agent", USER_AGENT.to_owned()),
            ("Content-Type", self.content_type.to_owned()),
            ("X-GitHub-Event", self.event.clone()),
            ("X-GitHub-Delivery", self.delivery.clone()),
            ("X-GitHub-Hook-ID", self.hook_id.to_string()),
            (
                "X-GitHub-Hook-Installation-Target-ID",
                self.target_id.to_string(),
            ),
            (
                "X-GitHub-Hook-Installation-Target-Type",
                self.target_type.to_owned(),
            ),
        ];
        if let Some(signature) = &self.signature {
            pairs.push(("X-Hub-Signature-256", signature.clone()));
        }
        pairs
    }
}

/// `sha256=` followed by the hex HMAC-SHA256 of `body` keyed with `secret`.
pub fn signature(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC accepts any key");
    mac.update(body);
    let digest = mac.finalize().into_bytes();
    format!(
        "sha256={}",
        digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    )
}

/// Records a delivery of `event` to every hook of `owner/name` that wants it and queues it.
/// A payload that fails validation is recorded with an error and not queued.
pub fn enqueue(
    state: &mut State,
    owner: &str,
    name: &str,
    event: &str,
    payload: &Value,
    only_hook: Option<u64>,
) {
    let Some(repo) = state.account(owner).and_then(|account| account.repo(name)) else {
        return;
    };
    let repository_id = repo.id;
    let mismatch = if state.validate_payloads {
        validate::check_webhook(event, payload).err()
    } else {
        None
    };
    if let Some(mismatch) = &mismatch {
        eprintln!("ninot-github: {mismatch}");
    }
    let targets: Vec<(u64, String, String, Option<String>)> = repo
        .hooks
        .iter()
        .filter(|hook| only_hook.is_none_or(|id| id == hook.id) && hook.wants(event))
        .map(|hook| {
            (
                hook.id,
                hook.url.clone(),
                hook.content_type.clone(),
                hook.secret.clone(),
            )
        })
        .collect();
    for (hook_id, url, content_type, secret) in targets {
        let json_body = serde_json::to_vec(payload).unwrap();
        let (body, mime) = if content_type == "form" {
            let form: String = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("payload", &String::from_utf8_lossy(&json_body))
                .finish();
            (form.into_bytes(), "application/x-www-form-urlencoded")
        } else {
            (json_body, "application/json")
        };
        let id = state.next_id();
        let headers = DeliveryHeaders {
            event: event.to_owned(),
            delivery: guid(id),
            hook_id,
            target_id: repository_id,
            target_type: "repository",
            content_type: mime,
            signature: secret
                .filter(|secret| !secret.is_empty())
                .map(|secret| signature(&secret, &body)),
        };
        state.deliveries.push(Delivery {
            id,
            guid: headers.delivery.clone(),
            hook_id,
            repository_id,
            event: event.to_owned(),
            url,
            headers,
            body,
            delivered_at: now(),
            status_code: None,
            error: mismatch.as_ref().map(|mismatch| {
                format!("ninot-github built a payload that does not match the spec: {mismatch}")
            }),
            duration_ms: 0,
        });
        if mismatch.is_none()
            && let Some(queue) = &state.webhook_queue
        {
            let _ = queue.send(id);
        }
    }
}

/// A delivery GUID in UUID format, derived from its unique id.
fn guid(id: u64) -> String {
    let hex = format!(
        "{:032x}",
        u128::from(id) | (0x6e69_6e6f_7400_0000u128 << 64)
    );
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// Which addresses a webhook delivery may contact.
///
/// [`TargetPolicy::Any`] is the default, so a delivery to `127.0.0.1` keeps working.
/// [`TargetPolicy::PublicOnly`] refuses loopback, link-local and private addresses.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TargetPolicy {
    /// Contact any resolved address, including loopback and private networks.
    #[default]
    Any,
    /// Contact an address only when it is publicly routable.
    PublicOnly,
}

impl TargetPolicy {
    /// Whether a delivery may contact `addr`.
    ///
    /// [`TargetPolicy::Any`] allows every address. [`TargetPolicy::PublicOnly`] refuses
    /// every address that is not globally routable: loopback, unspecified, private,
    /// carrier-grade NAT (`100.64/10`, which includes Tailscale), link-local, multicast,
    /// reserved and documentation ranges, IPv6 unique-local (`fc00::/7`, which includes
    /// Tailscale's `fd7a:115c:a1e0::/48`), and IPv4-mapped forms of any of these. 6to4
    /// (`2002::/16`) and Teredo (`2001::/32`) are refused wholesale, since they tunnel IPv4.
    pub fn allows(self, addr: std::net::IpAddr) -> bool {
        match self {
            Self::Any => true,
            Self::PublicOnly => !is_denied(addr),
        }
    }
}

/// Whether `addr` is not globally routable, including an IPv4-mapped IPv6 address whose
/// embedded IPv4 is not.
fn is_denied(addr: IpAddr) -> bool {
    match addr {
        IpAddr::V4(addr) => is_denied_v4(addr),
        IpAddr::V6(addr) => match addr.to_ipv4_mapped() {
            Some(mapped) => is_denied_v4(mapped),
            None => {
                let [first, second, ..] = addr.segments();
                addr.is_loopback()
                    || addr.is_unspecified()
                    || addr.is_multicast()
                    || (first & 0xffc0) == 0xfe80
                    || (first & 0xfe00) == 0xfc00
                    || (first, second) == (0x2001, 0x0db8)
                    || first == 0x2002
                    || (first, second) == (0x2001, 0x0000)
            }
        },
    }
}

/// Whether `addr` is in a range that is not globally routable.
fn is_denied_v4(addr: Ipv4Addr) -> bool {
    let [a, b, c, _] = addr.octets();
    addr.is_loopback()
        || addr.is_unspecified()
        || addr.is_private()
        || addr.is_link_local()
        || addr.is_broadcast()
        || addr.is_multicast()
        || (a == 100 && (b & 0xc0) == 64)
        || (a == 198 && (b & 0xfe) == 18)
        || (a & 0xf0) == 240
        || (a, b, c) == (192, 0, 2)
        || (a, b, c) == (198, 51, 100)
        || (a, b, c) == (203, 0, 113)
}

/// Webhook HTTP client settings: do not follow redirects, wait five seconds to connect,
/// and ten seconds for the whole request.
fn client_builder() -> reqwest::blocking::ClientBuilder {
    reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(10))
}

/// A client pinned to the addresses `url` resolves to, after [`TargetPolicy::PublicOnly`]
/// has accepted every one of them.
///
/// A resolution failure keeps the resolver's error text. A URL that cannot be checked, no
/// addresses, or any disallowed address is `webhook target not allowed`, and nothing is sent.
fn pinned_client(url: &str) -> Result<reqwest::blocking::Client, String> {
    let not_allowed = || "webhook target not allowed".to_owned();
    let parsed = url::Url::parse(url).map_err(|_| not_allowed())?;
    let Some(raw_host) = parsed.host_str() else {
        return Err(not_allowed());
    };
    // `host_str` keeps the brackets around an IPv6 literal; the resolver does not.
    let host = raw_host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(raw_host);
    let Some(port) = parsed.port_or_known_default() else {
        return Err(not_allowed());
    };
    let addrs: Vec<SocketAddr> = (host, port)
        .to_socket_addrs()
        .map_err(|error| error.to_string())?
        .collect();
    if addrs.is_empty()
        || addrs
            .iter()
            .any(|addr| !TargetPolicy::PublicOnly.allows(addr.ip()))
    {
        return Err(not_allowed());
    }
    client_builder()
        .no_proxy()
        .resolve_to_addrs(host, &addrs)
        .build()
        .map_err(|error| error.to_string())
}

/// POSTs `body` to `url` with `headers` and returns the status code, or the client error text.
fn post(
    client: &reqwest::blocking::Client,
    url: &str,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
) -> Result<u16, String> {
    let mut request = client.post(url).body(body);
    for (name, value) in headers {
        request = request.header(name, value);
    }
    request
        .send()
        .map(|response| response.status().as_u16())
        .map_err(|error| error.to_string())
}

/// Starts the thread that sends queued deliveries and records their outcome. It stops once
/// the state is gone.
pub fn spawn_sender(state: Weak<Mutex<State>>) -> mpsc::Sender<u64> {
    let (sender, receiver) = mpsc::channel::<u64>();
    std::thread::spawn(move || {
        let client = client_builder()
            .build()
            .expect("cannot build webhook client");
        for id in receiver {
            let Some(state) = state.upgrade() else {
                return;
            };
            let job = {
                let state = lock(&state);
                let policy = state.webhook_targets;
                state
                    .deliveries
                    .iter()
                    .find(|delivery| delivery.id == id)
                    .map(|delivery| {
                        (
                            delivery.url.clone(),
                            delivery.headers.pairs(),
                            delivery.body.clone(),
                            policy,
                        )
                    })
            };
            let Some((url, headers, body, policy)) = job else {
                continue;
            };
            let started = Instant::now();
            let outcome = match policy {
                TargetPolicy::Any => post(&client, &url, headers, body),
                TargetPolicy::PublicOnly => match pinned_client(&url) {
                    Ok(pinned) => post(&pinned, &url, headers, body),
                    Err(error) => Err(error),
                },
            };
            let mut state = lock(&state);
            if let Some(delivery) = state
                .deliveries
                .iter_mut()
                .find(|delivery| delivery.id == id)
            {
                delivery.duration_ms = started.elapsed().as_millis() as u64;
                match outcome {
                    Ok(status) => delivery.status_code = Some(status),
                    Err(error) => delivery.error = Some(error),
                }
            }
        }
    });
    sender
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::Hook;

    /// A state with `alice/tool` and one active hook on it.
    fn state_with_hook(validate_payloads: bool) -> State {
        let mut state = State::default();
        state.validate_payloads = validate_payloads;
        state.add_user("alice");
        let repo = state.create_repo("alice", "tool", false, None).unwrap();
        repo.hooks.push(Hook {
            id: 1,
            active: true,
            events: vec!["push".to_owned()],
            url: "http://127.0.0.1:1/hook".to_owned(),
            content_type: "json".to_owned(),
            secret: None,
            insecure_ssl: "0".to_owned(),
            created_at: now(),
            updated_at: now(),
        });
        state
    }

    #[test]
    fn mismatched_payload_is_recorded_with_an_error_and_not_queued() {
        let mut state = state_with_hook(true);
        let (queue, queued) = mpsc::channel();
        state.webhook_queue = Some(queue);
        enqueue(
            &mut state,
            "alice",
            "tool",
            "ping",
            &serde_json::json!({ "hook_id": "1" }),
            None,
        );
        let error = state.deliveries[0].error.as_deref().unwrap();
        assert!(error.contains("ping event does not match"), "{error}");
        assert!(queued.try_recv().is_err());
    }

    #[test]
    fn payloads_are_unchecked_without_validation() {
        let mut state = state_with_hook(false);
        enqueue(
            &mut state,
            "alice",
            "tool",
            "ping",
            &serde_json::json!({ "hook_id": "1" }),
            None,
        );
        assert_eq!(state.deliveries[0].error, None);
    }

    #[test]
    fn signature_matches_githubs_documented_example() {
        // From GitHub's "Validating webhook deliveries" documentation.
        assert_eq!(
            signature("It's a Secret to Everybody", b"Hello, World!"),
            "sha256=757107ea0eb2509fc211221cce984b8a37570b6d7586c22c46f4379c8b043e17"
        );
    }

    #[test]
    fn guids_look_like_uuids_and_are_unique() {
        let (a, b) = (guid(1), guid(2));
        assert_ne!(a, b);
        assert_eq!(a.len(), 36);
        assert_eq!(a.matches('-').count(), 4);
    }

    #[test]
    fn signature_header_only_with_a_secret() {
        let mut headers = DeliveryHeaders {
            event: "ping".to_owned(),
            delivery: guid(1),
            hook_id: 2,
            target_id: 3,
            target_type: "repository",
            content_type: "application/json",
            signature: None,
        };
        let names = |headers: &DeliveryHeaders| {
            headers
                .pairs()
                .into_iter()
                .map(|(name, _)| name)
                .collect::<Vec<_>>()
        };
        assert!(!names(&headers).contains(&"X-Hub-Signature-256"));
        headers.signature = Some("sha256=00".to_owned());
        assert!(names(&headers).contains(&"X-Hub-Signature-256"));
    }

    /// [`TargetPolicy::PublicOnly`] refuses every non-routable address in the table, and
    /// [`TargetPolicy::Any`] allows every one of them.
    #[test]
    fn a_public_only_policy_allows_only_public_addresses() {
        let cases = [
            // IPv4 private, loopback, link-local, unspecified, broadcast, and their edges.
            ("127.0.0.1", false),
            ("10.1.2.3", false),
            ("192.168.0.1", false),
            ("169.254.169.254", false),
            ("0.0.0.0", false),
            ("255.255.255.255", false),
            ("172.16.0.1", false),
            ("172.31.255.255", false),
            ("172.32.0.1", true),
            // Carrier-grade NAT 100.64/10, which holds the Tailscale range.
            ("100.64.0.0", false),
            ("100.100.100.100", false),
            ("100.127.255.255", false),
            ("100.63.255.255", true),
            ("100.128.0.0", true),
            // Multicast 224/4 and reserved 240/4.
            ("224.0.0.1", false),
            ("239.255.255.255", false),
            ("240.0.0.0", false),
            ("255.255.255.254", false),
            // Benchmarking 198.18/15.
            ("198.18.0.0", false),
            ("198.19.255.255", false),
            ("198.17.255.255", true),
            ("198.20.0.0", true),
            // Documentation ranges.
            ("192.0.2.1", false),
            ("192.0.1.255", true),
            ("198.51.100.7", false),
            ("198.51.101.0", true),
            ("203.0.113.9", false),
            ("203.0.114.0", true),
            // IPv6 loopback, unspecified, link-local, unique-local and Tailscale's prefix.
            ("::1", false),
            ("::", false),
            ("fe80::1", false),
            ("febf::1", false),
            ("fd00::1", false),
            ("fc00::1", false),
            ("fdff::1", false),
            ("fbff::1", true),
            ("fd7a:115c:a1e0::1", false),
            // IPv6 multicast, documentation, 6to4 and Teredo.
            ("ff00::1", false),
            ("ff02::1", false),
            ("2001:db8::1", false),
            ("2001:db9::1", true),
            ("2002::1", false),
            ("2001::1", false),
            ("2001:1::1", true),
            // IPv4-mapped forms are judged by their embedded IPv4.
            ("::ffff:127.0.0.1", false),
            ("::ffff:10.0.0.1", false),
            ("::ffff:169.254.169.254", false),
            ("::ffff:100.100.100.100", false),
            ("::ffff:93.184.216.34", true),
            // Public addresses.
            ("93.184.216.34", true),
            ("2606:4700::1111", true),
        ];
        for (text, allowed) in cases {
            let addr = text
                .parse::<std::net::IpAddr>()
                .unwrap_or_else(|_| panic!("{text} is not an address"));
            assert_eq!(
                TargetPolicy::PublicOnly.allows(addr),
                allowed,
                "{text} under PublicOnly"
            );
            assert!(TargetPolicy::Any.allows(addr), "{text} under Any");
        }
    }
}
