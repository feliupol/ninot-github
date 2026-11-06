//! Recording, signing and sending webhook deliveries.

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

/// Starts the thread that sends queued deliveries and records their outcome. It stops once
/// the state is gone.
pub fn spawn_sender(state: Weak<Mutex<State>>) -> mpsc::Sender<u64> {
    let (sender, receiver) = mpsc::channel::<u64>();
    std::thread::spawn(move || {
        let client = reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(10))
            .build()
            .expect("cannot build webhook client");
        for id in receiver {
            let Some(state) = state.upgrade() else {
                return;
            };
            let Some((url, headers, body)) = lock(&state)
                .deliveries
                .iter()
                .find(|delivery| delivery.id == id)
                .map(|delivery| {
                    (
                        delivery.url.clone(),
                        delivery.headers.pairs(),
                        delivery.body.clone(),
                    )
                })
            else {
                continue;
            };
            let started = Instant::now();
            let mut request = client.post(&url).body(body);
            for (name, value) in headers {
                request = request.header(name, value);
            }
            let outcome = request.send();
            let mut state = lock(&state);
            if let Some(delivery) = state
                .deliveries
                .iter_mut()
                .find(|delivery| delivery.id == id)
            {
                delivery.duration_ms = started.elapsed().as_millis() as u64;
                match outcome {
                    Ok(response) => delivery.status_code = Some(response.status().as_u16()),
                    Err(error) => delivery.error = Some(error.to_string()),
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
}
