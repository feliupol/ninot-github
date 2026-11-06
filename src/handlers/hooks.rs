//! Repository webhooks and their deliveries.

use std::sync::Mutex;

use url::Url;

use super::Handler;
use super::repos::{Found, find_repo};
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate};
use crate::operation::ops;
use crate::render;
use crate::state::{Hook, State, lock, now};
use crate::types;
use crate::webhooks;

/// Scopes that allow managing repository webhooks.
const HOOK_SCOPES: [&str; 3] = ["repo", "admin:repo_hook", "write:repo_hook"];

fn find_admin(state: &State, request: &ApiRequest) -> Result<Found, ApiResponse> {
    request.require_scope(&HOOK_SCOPES)?;
    let found = find_repo(state, request)?;
    if !found.permission.admin {
        return Err(ApiResponse::not_found());
    }
    Ok(found)
}

fn hooks<'a>(state: &'a State, found: &Found) -> &'a Vec<Hook> {
    &state
        .account(&found.owner)
        .unwrap()
        .repo(&found.name)
        .unwrap()
        .hooks
}

fn hooks_mut<'a>(state: &'a mut State, found: &Found) -> &'a mut Vec<Hook> {
    &mut state
        .account_mut(&found.owner)
        .unwrap()
        .repo_mut(&found.name)
        .unwrap()
        .hooks
}

fn hook_id(request: &ApiRequest) -> Result<u64, ApiResponse> {
    request
        .param("hook_id")
        .parse()
        .map_err(|_| ApiResponse::not_found())
}

fn render(request: &ApiRequest, found: &Found, hook: &Hook, state: &State) -> types::Hook {
    let api = format!(
        "{}/repos/{}/{}/hooks/{}",
        request.api_url, found.owner, found.name, hook.id
    );
    let last = state
        .deliveries
        .iter()
        .rev()
        .find(|delivery| delivery.hook_id == hook.id);
    let last_response = match last {
        None => types::HookResponse {
            code: None,
            message: None,
            status: Some("unused".to_owned()),
        },
        Some(delivery) => match delivery.status_code {
            Some(code) => types::HookResponse {
                code: Some(i64::from(code)),
                message: None,
                status: Some(
                    if (200..300).contains(&code) {
                        "active"
                    } else {
                        "failed"
                    }
                    .to_owned(),
                ),
            },
            None => types::HookResponse {
                code: None,
                message: delivery.error.clone(),
                status: Some("pending".to_owned()),
            },
        },
    };
    types::Hook {
        active: hook.active,
        config: config(hook),
        created_at: render::timestamp(&hook.created_at),
        deliveries_url: Some(format!("{api}/deliveries")),
        events: hook.events.clone(),
        id: render::id(hook.id),
        last_response,
        name: "web".to_owned(),
        ping_url: format!("{api}/pings"),
        test_url: format!("{api}/test"),
        type_: "Repository".to_owned(),
        updated_at: render::timestamp(&hook.updated_at),
        url: api,
    }
}

/// The hook configuration, with the secret masked as GitHub does.
fn config(hook: &Hook) -> types::WebhookConfig {
    types::WebhookConfig {
        content_type: Some(hook.content_type.clone().into()),
        insecure_ssl: Some(types::WebhookConfigInsecureSsl::String(
            hook.insecure_ssl.clone(),
        )),
        secret: hook.secret.as_ref().map(|_| "********".to_owned().into()),
        url: Some(hook.url.clone().into()),
    }
}

/// Applies `config` fields to `hook`, rejecting an invalid URL or content type.
fn apply_config(
    hook: &mut Hook,
    url: Option<&str>,
    content_type: Option<&str>,
    secret: Option<Option<&str>>,
    insecure_ssl: Option<String>,
) -> Result<(), ApiResponse> {
    if let Some(url) = url {
        if !is_web_url(url) {
            return Err(ApiResponse::validation_failed("Hook", "url", "invalid"));
        }
        hook.url = url.to_owned();
    }
    if let Some(content_type) = content_type {
        if !matches!(content_type, "json" | "form") {
            return Err(ApiResponse::validation_failed(
                "Hook",
                "content_type",
                "invalid",
            ));
        }
        hook.content_type = content_type.to_owned();
    }
    if let Some(secret) = secret {
        hook.secret = secret
            .map(str::to_owned)
            .filter(|secret| !secret.is_empty());
    }
    if let Some(insecure_ssl) = insecure_ssl {
        hook.insecure_ssl = insecure_ssl;
    }
    Ok(())
}

/// Whether a hook's target is an `http` or `https` URL, which is all a fake can deliver to.
fn is_web_url(url: &str) -> bool {
    Url::parse(url).is_ok_and(|url| matches!(url.scheme(), "http" | "https"))
}

/// The event names listed under `key`.
///
/// # Panics
///
/// If `key` holds anything but an array of strings, which the request schemas forbid.
impl Handler for ops::ReposCreateWebhook {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let body: Option<types::ReposCreateWebhookRequestBody> = request.json_as()?;
        let body = body.ok_or_else(|| ApiResponse::error(422, "Invalid request body"))?;
        let config = body.config.unwrap_or_default();
        let mut state = lock(state);
        let found = find_admin(&state, request)?;
        if config.url.is_none() {
            return Err(ApiResponse::validation_failed(
                "Hook",
                "url",
                "missing_field",
            ));
        }
        let created = now();
        let mut hook = Hook {
            id: state.next_id(),
            active: body.active,
            events: body.events,
            url: String::new(),
            content_type: "form".to_owned(),
            secret: None,
            insecure_ssl: "0".to_owned(),
            created_at: created.clone(),
            updated_at: created,
        };
        apply_config(
            &mut hook,
            config.url.as_ref().map(|value| value.0.as_str()),
            config.content_type.as_ref().map(|value| value.0.as_str()),
            config.secret.as_ref().map(|value| Some(value.0.as_str())),
            config.insecure_ssl.map(|value| value.to_string()),
        )?;
        let id = hook.id;
        hooks_mut(&mut state, &found).push(hook);
        let hook = hooks(&state, &found).last().unwrap();
        let rendered = render(request, &found, hook, &state);
        let payload = webhooks::ping_payload(request, &state, &found.owner, &found.name, &rendered);
        webhooks::enqueue(
            &mut state,
            &found.owner,
            &found.name,
            "ping",
            &payload,
            Some(id),
        );
        Ok(ApiResponse::typed(201, &rendered))
    }
}

impl Handler for ops::ReposListWebhooks {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_admin(&state, request)?;
        let items = hooks(&state, &found)
            .iter()
            .map(|hook| render(request, &found, hook, &state))
            .collect();
        Ok(paginate(request, &request.path, items))
    }
}

impl Handler for ops::ReposGetWebhook {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let id = hook_id(request)?;
        let state = lock(state);
        let found = find_admin(&state, request)?;
        let hook = hooks(&state, &found)
            .iter()
            .find(|hook| hook.id == id)
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::typed(
            200,
            &render(request, &found, hook, &state),
        ))
    }
}

impl Handler for ops::ReposUpdateWebhook {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let id = hook_id(request)?;
        let body: types::ReposUpdateWebhookRequest = request.json_as()?;
        let mut state = lock(state);
        let found = find_admin(&state, request)?;
        let stored = hooks_mut(&mut state, &found)
            .iter_mut()
            .find(|hook| hook.id == id)
            .ok_or_else(ApiResponse::not_found)?;
        let mut hook = stored.clone();
        if let Some(config) = body.config {
            apply_config(
                &mut hook,
                config.url.as_ref().map(|value| value.0.as_str()),
                config.content_type.as_ref().map(|value| value.0.as_str()),
                config
                    .secret
                    .as_ref()
                    .map(|value| value.as_ref().map(|value| value.0.as_str())),
                config.insecure_ssl.map(|value| value.to_string()),
            )?;
        }
        if let Some(events) = body.events {
            hook.events = events;
        }
        for event in body.add_events.unwrap_or_default() {
            if !hook.events.contains(&event) {
                hook.events.push(event);
            }
        }
        if let Some(removed) = body.remove_events {
            hook.events.retain(|event| !removed.contains(event));
        }
        if let Some(active) = body.active {
            hook.active = active;
        }
        hook.updated_at = now();
        *stored = hook;
        let hook = hooks(&state, &found)
            .iter()
            .find(|hook| hook.id == id)
            .unwrap();
        Ok(ApiResponse::typed(
            200,
            &render(request, &found, hook, &state),
        ))
    }
}

impl Handler for ops::ReposDeleteWebhook {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let id = hook_id(request)?;
        let mut state = lock(state);
        let found = find_admin(&state, request)?;
        let hooks = hooks_mut(&mut state, &found);
        let index = hooks
            .iter()
            .position(|hook| hook.id == id)
            .ok_or_else(ApiResponse::not_found)?;
        hooks.remove(index);
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::ReposPingWebhook {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let id = hook_id(request)?;
        let mut state = lock(state);
        let found = find_admin(&state, request)?;
        let hook = hooks(&state, &found)
            .iter()
            .find(|hook| hook.id == id)
            .ok_or_else(ApiResponse::not_found)?;
        let rendered = render(request, &found, hook, &state);
        let payload = webhooks::ping_payload(request, &state, &found.owner, &found.name, &rendered);
        webhooks::enqueue(
            &mut state,
            &found.owner,
            &found.name,
            "ping",
            &payload,
            Some(id),
        );
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::ReposListWebhookDeliveries {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let id = hook_id(request)?;
        let state = lock(state);
        let found = find_admin(&state, request)?;
        let repository_id = state
            .account(&found.owner)
            .unwrap()
            .repo(&found.name)
            .unwrap()
            .id;
        if !hooks(&state, &found).iter().any(|hook| hook.id == id) {
            return Err(ApiResponse::not_found());
        }
        let items = state
            .deliveries
            .iter()
            .rev()
            .filter(|delivery| delivery.hook_id == id)
            .map(|delivery| {
                let (status, code) = match (delivery.status_code, &delivery.error) {
                    (Some(code), _) if (200..300).contains(&code) => ("OK".to_owned(), code),
                    (Some(code), _) => (format!("Invalid HTTP Response: {code}"), code),
                    (None, Some(error)) => (error.clone(), 0),
                    (None, None) => ("Pending".to_owned(), 0),
                };
                types::HookDeliveryItem {
                    action: None,
                    delivered_at: render::timestamp(&delivery.delivered_at),
                    duration: delivery.duration_ms as f64 / 1000.0,
                    event: delivery.event.clone(),
                    guid: delivery.guid.clone(),
                    id: render::id(delivery.id),
                    installation_id: None,
                    redelivery: false,
                    repository_id: Some(render::id(repository_id)),
                    status,
                    status_code: i64::from(code),
                    throttled_at: None,
                }
            })
            .collect::<Vec<_>>();
        Ok(ApiResponse::typed(200, &items))
    }
}
