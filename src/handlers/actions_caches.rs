//! GitHub Actions caches.

use std::sync::Mutex;

use super::Handler;
use super::actions_workflows::require_actions_write;
use super::repos::find_repo;
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::operation::ops;
use crate::render;
use crate::state::{CacheRecord, State, lock};
use crate::types;

fn render_cache(cache: &CacheRecord) -> types::ActionsCacheListActionsCachesItem {
    types::ActionsCacheListActionsCachesItem {
        created_at: Some(cache.created_at),
        id: Some(render::id(cache.id)),
        key: Some(cache.key.clone()),
        last_accessed_at: Some(cache.last_accessed_at),
        ref_: Some(cache.git_ref.clone()),
        size_in_bytes: Some(cache.size_in_bytes),
        version: Some(cache.version.clone()),
    }
}

fn cache_list(request: &ApiRequest, caches: &[&CacheRecord]) -> ApiResponse {
    let mut caches: Vec<&CacheRecord> = caches.to_vec();
    let sort = request.query("sort").unwrap_or("last_accessed_at");
    let ascending = request
        .query("direction")
        .is_some_and(|value| value == "asc");
    caches.sort_by(|left, right| {
        let order = match sort {
            "created_at" => left.created_at.cmp(&right.created_at),
            "size_in_bytes" => left.size_in_bytes.cmp(&right.size_in_bytes),
            _ => left.last_accessed_at.cmp(&right.last_accessed_at),
        }
        .then(left.id.cmp(&right.id));
        if ascending { order } else { order.reverse() }
    });
    let rendered = caches.into_iter().map(render_cache).collect::<Vec<_>>();
    crate::api::paginate_with(
        request,
        &request.path,
        rendered,
        |total_count, actions_caches| {
            types::ActionsGetActionsCacheListResponse(types::ActionsCacheList {
                actions_caches,
                total_count,
            })
        },
    )
}

fn matches(cache: &CacheRecord, request: &ApiRequest) -> bool {
    request
        .query("key")
        .is_none_or(|prefix| cache.key.starts_with(prefix))
        && request
            .query("ref")
            .is_none_or(|git_ref| cache.git_ref == git_ref)
}

impl Handler for ops::ActionsGetActionsCacheList {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find_repo(&state, request)?;
        let repo = found.repo(&state);
        let caches = repo
            .caches
            .iter()
            .filter(|cache| matches(cache, request))
            .collect::<Vec<_>>();
        Ok(cache_list(request, &caches))
    }
}

impl Handler for ops::ActionsDeleteActionsCacheById {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let mut state = lock(state);
        let found = require_actions_write(&state, request)?;
        let id = request
            .param("cache_id")
            .parse::<u64>()
            .map_err(|_| ApiResponse::not_found())?;
        let repo = found.repo_mut(&mut state);
        let Some(index) = repo.caches.iter().position(|cache| cache.id == id) else {
            return Err(ApiResponse::not_found());
        };
        repo.caches.remove(index);
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::ActionsDeleteActionsCacheByKey {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let mut state = lock(state);
        let found = require_actions_write(&state, request)?;
        let key = request
            .query("key")
            .ok_or_else(|| ApiResponse::unprocessable("key is required"))?
            .to_owned();
        let git_ref = request.query("ref").map(str::to_owned);
        let repo = found.repo_mut(&mut state);
        let matched: Vec<CacheRecord> = repo
            .caches
            .iter()
            .filter(|cache| {
                cache.key == key
                    && git_ref
                        .as_ref()
                        .is_none_or(|git_ref| &cache.git_ref == git_ref)
            })
            .cloned()
            .collect();
        if matched.is_empty() {
            return Err(ApiResponse::not_found());
        }
        repo.caches.retain(|cache| {
            !(cache.key == key
                && git_ref
                    .as_ref()
                    .is_none_or(|git_ref| &cache.git_ref == git_ref))
        });
        let rendered = matched.iter().map(render_cache).collect();
        Ok(ApiResponse::typed(
            200,
            &types::ActionsDeleteActionsCacheByKeyResponse(types::ActionsCacheList {
                total_count: matched.len() as i64,
                actions_caches: rendered,
            }),
        ))
    }
}
