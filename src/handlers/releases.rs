//! Releases and their assets.

use std::sync::Mutex;

use sha2::{Digest, Sha256};

use super::Handler;
use super::repos::{Found, find_repo};
use crate::api::{ApiRequest, ApiResponse, ApiResult, Caller, paginate};
use crate::operation::ops;
use crate::render::{self, node_id};
use crate::state::{Asset, Release, Repo, State, lock, now};
use crate::types;
use ninot_common::git;

/// Finds the repository and requires push access when `write` is set.
fn find(state: &State, request: &ApiRequest, write: bool) -> Result<Found, ApiResponse> {
    let found = find_repo(state, request)?;
    if write && !found.permission.push {
        return Err(ApiResponse::error(
            403,
            "Must have push access to repository",
        ));
    }
    Ok(found)
}

fn repo<'a>(state: &'a State, found: &Found) -> &'a Repo {
    state
        .account(&found.owner)
        .unwrap()
        .repo(&found.name)
        .unwrap()
}

fn repo_mut<'a>(state: &'a mut State, found: &Found) -> &'a mut Repo {
    state
        .account_mut(&found.owner)
        .unwrap()
        .repo_mut(&found.name)
        .unwrap()
}

fn render_asset(
    state: &State,
    request: &ApiRequest,
    found: &Found,
    release: &Release,
    asset: &Asset,
) -> types::ReleaseAsset {
    let api = format!("{}/repos/{}/{}", request.api_url, found.owner, found.name);
    let created = render::timestamp(&asset.created_at);
    types::ReleaseAsset {
        browser_download_url: format!(
            "{}/{}/{}/releases/download/{}/{}",
            request.web_url, found.owner, found.name, release.tag_name, asset.name
        ),
        content_type: asset.content_type.clone(),
        created_at: created,
        digest: Some(asset.digest.clone()),
        download_count: 0,
        id: render::id(asset.id),
        label: asset.label.clone(),
        name: asset.name.clone(),
        node_id: node_id("ReleaseAsset", asset.id),
        size: asset.data.len() as i64,
        state: types::ReleaseAssetState::Uploaded,
        updated_at: created,
        uploader: state
            .account(&asset.uploader)
            .map(|account| render::nullable_simple_user(request, account)),
        url: format!("{api}/releases/assets/{}", asset.id),
    }
}

fn render_release(
    state: &State,
    request: &ApiRequest,
    found: &Found,
    release: &Release,
) -> types::Release {
    let api = format!("{}/repos/{}/{}", request.api_url, found.owner, found.name);
    let html = format!("{}/{}/{}", request.web_url, found.owner, found.name);
    let author = state.account(&release.author).unwrap();
    types::Release {
        assets: release
            .assets
            .iter()
            .map(|asset| render_asset(state, request, found, release, asset))
            .collect(),
        assets_url: format!("{api}/releases/{}/assets", release.id),
        author: render::simple_user(request, author),
        body: release.body.clone(),
        body_html: None,
        body_text: None,
        created_at: render::timestamp(&release.created_at),
        discussion_url: None,
        draft: release.draft,
        html_url: format!("{html}/releases/tag/{}", release.tag_name),
        id: render::id(release.id),
        immutable: Some(false),
        mentions_count: None,
        name: release.name.clone(),
        node_id: node_id("Release", release.id),
        prerelease: release.prerelease,
        published_at: release.published_at.as_deref().map(render::timestamp),
        reactions: None,
        tag_name: release.tag_name.clone(),
        tarball_url: Some(format!("{api}/tarball/{}", release.tag_name)),
        target_commitish: release.target_commitish.clone(),
        updated_at: None,
        upload_url: format!(
            "{}/api/uploads/repos/{}/{}/releases/{}/assets{{?name,label}}",
            request.web_url, found.owner, found.name, release.id
        ),
        url: format!("{api}/releases/{}", release.id),
        zipball_url: Some(format!("{api}/zipball/{}", release.tag_name)),
    }
}

/// Releases the caller may see: drafts only with push access, newest first.
fn visible<'a>(repo: &'a Repo, found: &Found) -> impl Iterator<Item = &'a Release> {
    let push = found.permission.push;
    repo.releases
        .iter()
        .rev()
        .filter(move |release| push || !release.draft)
}

fn id_param(request: &ApiRequest, name: &str) -> Result<u64, ApiResponse> {
    request
        .param(name)
        .parse()
        .map_err(|_| ApiResponse::not_found())
}

/// Creates `refs/tags/{tag}` at `target` unless the tag exists.
///
/// Returns whether this call created the tag ref. A missing git directory creates nothing.
fn ensure_tag(repo: &Repo, tag: &str, target: &str) -> Result<bool, ApiResponse> {
    let Some(git_dir) = &repo.git_dir else {
        return Ok(false);
    };
    let internal = |error: std::io::Error| ApiResponse::error(500, &error.to_string());
    if git::resolve_commit(git_dir, &format!("refs/tags/{tag}"))
        .map_err(internal)?
        .is_some()
    {
        return Ok(false);
    }
    if git::is_empty(git_dir).map_err(internal)? {
        return Err(ApiResponse::error(422, "Repository is empty."));
    }
    let sha = git::resolve_commit(git_dir, target)
        .map_err(internal)?
        .ok_or_else(|| ApiResponse::validation_failed("Release", "target_commitish", "invalid"))?;
    git::create_ref(git_dir, &format!("refs/tags/{tag}"), &sha, &super::BOT).map_err(internal)?;
    Ok(true)
}

impl Handler for ops::ReposCreateRelease {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?.clone();
        let body: types::ReposCreateReleaseRequest = request.json_as()?;
        let tag_name = body.tag_name;
        if tag_name.is_empty() {
            return Err(ApiResponse::validation_failed(
                "Release",
                "tag_name",
                "missing_field",
            ));
        };
        let draft = body.draft;
        let prerelease = body.prerelease;
        let mut state = lock(state);
        let found = find(&state, request, true)?;
        let current = repo(&state, &found);
        if current
            .releases
            .iter()
            .any(|release| release.tag_name == tag_name)
        {
            return Err(ApiResponse::validation_failed(
                "Release",
                "tag_name",
                "already_exists",
            ));
        }
        let target = body
            .target_commitish
            .unwrap_or_else(|| current.default_branch.clone());
        let created_tag = if draft {
            false
        } else {
            ensure_tag(current, &tag_name, &target)?
        };
        let id = state.next_id();
        let created = now();
        let release = Release {
            id,
            tag_name,
            target_commitish: target,
            name: body.name,
            body: body.body,
            draft,
            prerelease,
            author: caller.login,
            created_at: created.clone(),
            published_at: (!draft).then_some(created),
            assets: Vec::new(),
        };
        repo_mut(&mut state, &found).releases.push(release);
        let rendered = render_release(
            &state,
            request,
            &found,
            repo(&state, &found).releases.last().unwrap(),
        );
        let action = if rendered.draft {
            "created"
        } else if rendered.prerelease {
            "prereleased"
        } else {
            "published"
        };
        let mut ctx =
            crate::webhooks::events::Ctx::new(&mut state, request, &found.owner, &found.name);
        if created_tag {
            crate::webhooks::events::create::reference(&mut ctx, &rendered.tag_name, "tag");
        }
        crate::webhooks::events::release::deliver(&mut ctx, action, &rendered, None);
        Ok(ApiResponse::typed(201, &rendered))
    }
}

impl Handler for ops::ReposListReleases {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find(&state, request, false)?;
        let items = visible(repo(&state, &found), &found)
            .map(|release| render_release(&state, request, &found, release))
            .collect();
        Ok(paginate(request, &request.path, items))
    }
}

impl Handler for ops::ReposGetLatestRelease {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find(&state, request, false)?;
        let release = repo(&state, &found)
            .releases
            .iter()
            .rev()
            .find(|release| !release.draft && !release.prerelease)
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::typed(
            200,
            &render_release(&state, request, &found, release),
        ))
    }
}

impl Handler for ops::ReposGetReleaseByTag {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = find(&state, request, false)?;
        let tag = request.param("tag");
        let release = repo(&state, &found)
            .releases
            .iter()
            .find(|release| !release.draft && release.tag_name == tag)
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::typed(
            200,
            &render_release(&state, request, &found, release),
        ))
    }
}

impl Handler for ops::ReposGetRelease {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let id = id_param(request, "release_id")?;
        let state = lock(state);
        let found = find(&state, request, false)?;
        let release = visible(repo(&state, &found), &found)
            .find(|release| release.id == id)
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::typed(
            200,
            &render_release(&state, request, &found, release),
        ))
    }
}

impl Handler for ops::ReposUpdateRelease {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_scope(&["repo", "public_repo"])?;
        let id = id_param(request, "release_id")?;
        let body: types::ReposUpdateReleaseRequest = request.json_as()?;
        let mut state = lock(state);
        let found = find(&state, request, true)?;
        let current = repo(&state, &found);
        let index = current
            .releases
            .iter()
            .position(|release| release.id == id)
            .ok_or_else(ApiResponse::not_found)?;
        let release = &current.releases[index];
        let tag = body
            .tag_name
            .clone()
            .unwrap_or_else(|| release.tag_name.clone());
        let target = body
            .target_commitish
            .clone()
            .unwrap_or_else(|| release.target_commitish.clone());
        let was_draft = release.draft;
        let old_name = release.name.clone().unwrap_or_default();
        let old_body = release.body.clone().unwrap_or_default();
        let draft = body.draft.unwrap_or(release.draft);
        if current
            .releases
            .iter()
            .any(|other| other.id != id && other.tag_name == tag)
        {
            return Err(ApiResponse::validation_failed(
                "Release",
                "tag_name",
                "already_exists",
            ));
        }
        let created_tag = if draft {
            false
        } else {
            ensure_tag(current, &tag, &target)?
        };
        let release = &mut repo_mut(&mut state, &found).releases[index];
        if release.draft && !draft {
            release.published_at = Some(now());
        }
        release.tag_name = tag;
        release.target_commitish = target;
        release.draft = draft;
        if let Some(prerelease) = body.prerelease {
            release.prerelease = prerelease;
        }
        if let Some(name) = body.name {
            release.name = Some(name);
        }
        if let Some(text) = body.body {
            release.body = Some(text);
        }
        let rendered = render_release(
            &state,
            request,
            &found,
            &repo(&state, &found).releases[index],
        );
        let mut ctx =
            crate::webhooks::events::Ctx::new(&mut state, request, &found.owner, &found.name);
        if created_tag {
            crate::webhooks::events::create::reference(&mut ctx, &rendered.tag_name, "tag");
        }
        if was_draft && !rendered.draft {
            let action = if rendered.prerelease {
                "prereleased"
            } else {
                "published"
            };
            crate::webhooks::events::release::deliver(&mut ctx, action, &rendered, None);
        } else if !was_draft && rendered.draft {
            crate::webhooks::events::release::deliver(&mut ctx, "unpublished", &rendered, None);
        } else {
            let new_name = rendered.name.clone().unwrap_or_default();
            let new_body = rendered.body.clone().unwrap_or_default();
            let mut changes = serde_json::Map::new();
            if old_name != new_name {
                changes.insert("name".to_owned(), serde_json::json!({ "from": old_name }));
            }
            if old_body != new_body {
                changes.insert("body".to_owned(), serde_json::json!({ "from": old_body }));
            }
            if !changes.is_empty() {
                crate::webhooks::events::release::deliver(
                    &mut ctx,
                    "edited",
                    &rendered,
                    Some(changes),
                );
            }
        }
        Ok(ApiResponse::typed(200, &rendered))
    }
}

impl Handler for ops::ReposDeleteRelease {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_scope(&["repo", "public_repo"])?;
        let id = id_param(request, "release_id")?;
        let mut state = lock(state);
        let found = find(&state, request, true)?;
        let rendered = {
            let release = repo(&state, &found)
                .releases
                .iter()
                .find(|release| release.id == id)
                .ok_or_else(ApiResponse::not_found)?;
            render_release(&state, request, &found, release)
        };
        repo_mut(&mut state, &found)
            .releases
            .retain(|release| release.id != id);
        let mut ctx =
            crate::webhooks::events::Ctx::new(&mut state, request, &found.owner, &found.name);
        crate::webhooks::events::release::deliver(&mut ctx, "deleted", &rendered, None);
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::ReposListReleaseAssets {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let id = id_param(request, "release_id")?;
        let state = lock(state);
        let found = find(&state, request, false)?;
        let release = visible(repo(&state, &found), &found)
            .find(|release| release.id == id)
            .ok_or_else(ApiResponse::not_found)?;
        let items = release
            .assets
            .iter()
            .map(|asset| render_asset(&state, request, &found, release, asset))
            .collect();
        Ok(paginate(request, &request.path, items))
    }
}

impl Handler for ops::ReposUploadReleaseAsset {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let caller = request.require_scope(&["repo", "public_repo"])?.clone();
        let id = id_param(request, "release_id")?;
        let Some(name) = request
            .query("name")
            .filter(|name| !name.is_empty())
            .map(str::to_owned)
        else {
            return Err(ApiResponse::validation_failed(
                "ReleaseAsset",
                "name",
                "missing_field",
            ));
        };
        let content_type = request
            .headers
            .get(http::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_owned();
        let mut state = lock(state);
        let found = find(&state, request, true)?;
        let asset_id = state.next_id();
        let release = repo_mut(&mut state, &found)
            .releases
            .iter_mut()
            .find(|release| release.id == id)
            .ok_or_else(ApiResponse::not_found)?;
        if release.assets.iter().any(|asset| asset.name == name) {
            return Err(ApiResponse::validation_failed(
                "ReleaseAsset",
                "name",
                "already_exists",
            ));
        }
        release.assets.push(Asset {
            id: asset_id,
            name,
            label: request.query("label").map(str::to_owned),
            content_type,
            digest: format!("sha256:{:x}", Sha256::digest(&request.body)),
            data: request.body.to_vec(),
            uploader: caller.login,
            created_at: now(),
        });
        let release = repo(&state, &found)
            .releases
            .iter()
            .find(|release| release.id == id)
            .unwrap();
        let asset = release.assets.last().unwrap();
        Ok(ApiResponse::typed(
            201,
            &render_asset(&state, request, &found, release, asset),
        ))
    }
}

/// Finds an asset by id among the releases the caller can see.
fn find_asset<'a>(repo: &'a Repo, found: &Found, id: u64) -> Option<(&'a Release, &'a Asset)> {
    visible(repo, found).find_map(|release| {
        release
            .assets
            .iter()
            .find(|asset| asset.id == id)
            .map(|asset| (release, asset))
    })
}

/// Returns the metadata, or the bytes with `Accept: application/octet-stream`.
impl Handler for ops::ReposGetReleaseAsset {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let id = id_param(request, "asset_id")?;
        let state = lock(state);
        let found = find(&state, request, false)?;
        let (release, asset) =
            find_asset(repo(&state, &found), &found, id).ok_or_else(ApiResponse::not_found)?;
        if request.accept().contains("application/octet-stream") {
            return Ok(ApiResponse::bytes(
                200,
                &asset.content_type,
                asset.data.clone(),
            ));
        }
        Ok(ApiResponse::typed(
            200,
            &render_asset(&state, request, &found, release, asset),
        ))
    }
}

impl Handler for ops::ReposDeleteReleaseAsset {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        request.require_scope(&["repo", "public_repo"])?;
        let id = id_param(request, "asset_id")?;
        let mut state = lock(state);
        let found = find(&state, request, true)?;
        for release in &mut repo_mut(&mut state, &found).releases {
            if let Some(index) = release.assets.iter().position(|asset| asset.id == id) {
                release.assets.remove(index);
                return Ok(ApiResponse::no_content());
            }
        }
        Err(ApiResponse::not_found())
    }
}

/// Serves `/{owner}/{repo}/releases/download/{tag}/{name}`, the `browser_download_url`.
pub fn download(state: &Mutex<State>, caller: Option<&Caller>, path: &str) -> Option<ApiResponse> {
    let parts: Vec<&str> = path.strip_prefix('/')?.split('/').collect();
    let [owner, name, "releases", "download", tag @ .., file] = &parts[..] else {
        return None;
    };
    if tag.is_empty() {
        return None;
    }
    let tag = tag.join("/");
    let state = lock(state);
    let Some(permission) = state.permission(caller, owner, name) else {
        return Some(ApiResponse::not_found());
    };
    let repo = state.account(owner)?.repo(name)?;
    let asset = repo
        .releases
        .iter()
        .filter(|release| permission.push || !release.draft)
        .find(|release| release.tag_name == tag)
        .and_then(|release| release.assets.iter().find(|asset| asset.name == *file));
    Some(match asset {
        Some(asset) => ApiResponse::bytes(200, "application/octet-stream", asset.data.clone()),
        None => ApiResponse::not_found(),
    })
}
