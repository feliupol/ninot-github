//! The `ping` event, sent when a hook is created or pinged.

use serde_json::Value;

use crate::api::ApiRequest;
use crate::render;
use crate::state::{Permission, State};
use crate::types;
use crate::webhook_types;

/// The payload of `ping` for `hook`, rendered as the hook API returns it.
pub fn ping_payload(
    request: &ApiRequest,
    state: &State,
    owner: &str,
    name: &str,
    hook: &types::Hook,
) -> Value {
    let account = state.account(owner).unwrap();
    let repository = render::full_repository(
        request,
        account,
        account.repo(name).unwrap(),
        Permission::default(),
    );
    let payload = webhook_types::PingEvent {
        hook: ping_hook(hook),
        hook_id: hook.id,
        organization: None,
        repository: Some(super::repository(&repository)),
        sender: request
            .caller
            .as_ref()
            .and_then(|caller| state.account(&caller.login))
            .map(|sender| render::simple_user(request, sender).into()),
        zen: "Keep it logically awesome.".to_owned(),
    };
    serde_json::to_value(payload).expect("payloads serialize to JSON")
}

/// The `hook` of a `ping` payload. octokit/webhooks describes only the `last_response` of a
/// hook without responses, whose `code` and `message` are null, so other hooks omit it.
fn ping_hook(hook: &types::Hook) -> webhook_types::PingEventHook {
    let config = &hook.config;
    let insecure_ssl = match config.insecure_ssl.as_ref() {
        Some(types::WebhookConfigInsecureSsl::String(value)) => value.as_str(),
        _ => "0",
    };
    let events = if hook.events.len() == 1 {
        webhook_types::WebhookEvents::Variant1([hook.events[0].clone()])
    } else {
        webhook_types::WebhookEvents::Variant0(
            hook.events
                .iter()
                .map(|event| {
                    event
                        .parse()
                        .expect("configured hook events match the webhook schema")
                })
                .collect(),
        )
    };
    webhook_types::PingEventHook {
        active: hook.active,
        app_id: None,
        config: webhook_types::PingEventHookConfig {
            content_type: match config.content_type.as_ref().map(|value| value.0.as_str()) {
                Some("json") => webhook_types::PingEventHookConfigContentType::Json,
                _ => webhook_types::PingEventHookConfigContentType::Form,
            },
            insecure_ssl: if insecure_ssl == "1" {
                webhook_types::PingEventHookConfigInsecureSsl::X1
            } else {
                webhook_types::PingEventHookConfigInsecureSsl::X0
            },
            secret: config.secret.as_ref().map(|value| value.0.clone()),
            url: config
                .url
                .as_ref()
                .map(|value| value.0.clone())
                .unwrap_or_default(),
        },
        created_at: hook.created_at,
        deliveries_url: hook.deliveries_url.clone().unwrap_or_default(),
        events,
        id: hook.id,
        last_response: if hook.last_response.code.is_none() && hook.last_response.message.is_none()
        {
            Some(webhook_types::PingEventHookLastResponse {
                code: (),
                message: (),
                status: hook
                    .last_response
                    .status
                    .clone()
                    .unwrap_or_else(|| "unused".to_owned()),
            })
        } else {
            None
        },
        name: hook.name.clone(),
        ping_url: hook.ping_url.clone(),
        test_url: Some(hook.test_url.clone()),
        type_: match hook.type_.as_str() {
            "Organization" => webhook_types::PingEventHookType::Organization,
            "App" => webhook_types::PingEventHookType::App,
            _ => webhook_types::PingEventHookType::Repository,
        },
        updated_at: hook.updated_at,
        url: hook.url.clone(),
    }
}
