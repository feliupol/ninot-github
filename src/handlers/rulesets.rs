//! Repository rulesets. They are stored and served back; pushes do not enforce them.

use std::sync::Mutex;

use super::Handler;
use super::repo_admin::{admin_repo, require_rulesets};
use super::repos::Found;
use crate::api::{ApiRequest, ApiResponse, ApiResult, paginate};
use crate::operation::ops;
use crate::render::node_id;
use crate::state::{Ruleset, State, lock};
use crate::types;

fn target(value: types::ReposCreateRepoRulesetRequestTarget) -> types::RepositoryRulesetTarget {
    match value {
        types::ReposCreateRepoRulesetRequestTarget::Branch => {
            types::RepositoryRulesetTarget::Branch
        }
        types::ReposCreateRepoRulesetRequestTarget::Tag => types::RepositoryRulesetTarget::Tag,
        types::ReposCreateRepoRulesetRequestTarget::Push => types::RepositoryRulesetTarget::Push,
    }
}

fn update_target(
    value: types::ReposUpdateRepoRulesetRequestTarget,
) -> types::RepositoryRulesetTarget {
    match value {
        types::ReposUpdateRepoRulesetRequestTarget::Branch => {
            types::RepositoryRulesetTarget::Branch
        }
        types::ReposUpdateRepoRulesetRequestTarget::Tag => types::RepositoryRulesetTarget::Tag,
        types::ReposUpdateRepoRulesetRequestTarget::Push => types::RepositoryRulesetTarget::Push,
    }
}

/// A ruleset as `GET /repos/{owner}/{repo}/rulesets/{ruleset_id}` returns it.
fn render(request: &ApiRequest, found: &Found, ruleset: &Ruleset) -> types::RepositoryRuleset {
    let urls = request.urls().repo(&found.owner, &found.name);
    types::RepositoryRuleset {
        bypass_actors: ruleset.bypass_actors.clone(),
        conditions: ruleset.conditions.clone(),
        created_at: Some(ruleset.created_at),
        // The caller administers the repository.
        current_user_can_bypass: Some(types::RepositoryRulesetCurrentUserCanBypass::Never),
        enforcement: ruleset.enforcement,
        id: ruleset.id,
        // Source: OpenAPI repository-ruleset example, `_links`.
        links: Some(types::RepositoryRulesetLinks {
            html: Some(types::RepositoryRulesetLinksHtml {
                href: Some(format!("{}/rules/{}", urls.html, ruleset.id)),
            }),
            self_: Some(types::RepositoryRulesetLinksSelf {
                href: Some(urls.api(&format!("/rulesets/{}", ruleset.id))),
            }),
        }),
        name: ruleset.name.clone(),
        node_id: Some(node_id("RepositoryRuleset", ruleset.id as u64)),
        rules: ruleset.rules.clone(),
        source: format!("{}/{}", found.owner, found.name),
        source_type: Some(types::RepositoryRulesetSourceType::Repository),
        target: Some(ruleset.target),
        updated_at: Some(ruleset.updated_at),
    }
}

/// A ruleset as the list returns it: without conditions and rules.
// Source: OpenAPI repository-ruleset-items example.
fn render_item(request: &ApiRequest, found: &Found, ruleset: &Ruleset) -> types::RepositoryRuleset {
    types::RepositoryRuleset {
        bypass_actors: Vec::new(),
        conditions: None,
        rules: Vec::new(),
        current_user_can_bypass: None,
        ..render(request, found, ruleset)
    }
}

fn duplicate_name() -> ApiResponse {
    ApiResponse::validation_failed("Ruleset", "name", "already_exists")
}

/// The administered repository of the request, when its plan has rulesets.
fn ruleset_repo(state: &State, request: &ApiRequest) -> Result<Found, ApiResponse> {
    let found = admin_repo(state, request)?;
    require_rulesets(state, &found)?;
    Ok(found)
}

fn ruleset_id(request: &ApiRequest) -> Result<i64, ApiResponse> {
    request
        .param("ruleset_id")
        .parse()
        .map_err(|_| ApiResponse::not_found())
}

impl Handler for ops::ReposGetRepoRulesets {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let found = ruleset_repo(&state, request)?;
        let items = found
            .repo(&state)
            .setup
            .rulesets
            .values()
            .map(|ruleset| render_item(request, &found, ruleset))
            .collect::<Vec<_>>();
        Ok(paginate(request, &request.path, items))
    }
}

impl Handler for ops::ReposGetRepoRuleset {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let id = ruleset_id(request)?;
        let state = lock(state);
        let found = ruleset_repo(&state, request)?;
        let ruleset = found
            .repo(&state)
            .setup
            .rulesets
            .get(&id)
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::ok_typed(&types::ReposGetRepoRulesetResponse(
            render(request, &found, ruleset),
        )))
    }
}

impl Handler for ops::ReposCreateRepoRuleset {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let body: types::ReposCreateRepoRulesetRequest = request.json_as()?;
        let mut state = lock(state);
        let found = ruleset_repo(&state, request)?;
        if found
            .repo(&state)
            .setup
            .rulesets
            .values()
            .any(|ruleset| ruleset.name == body.name)
        {
            return Err(duplicate_name());
        }
        let id = state.next_id() as i64;
        let now = crate::time::now_utc();
        let ruleset = Ruleset {
            id,
            name: body.name,
            target: target(body.target),
            enforcement: body.enforcement,
            bypass_actors: body.bypass_actors,
            conditions: body.conditions,
            rules: body.rules,
            created_at: now,
            updated_at: now,
        };
        let rendered = render(request, &found, &ruleset);
        found
            .repo_mut(&mut state)
            .setup
            .rulesets
            .insert(id, ruleset);
        Ok(ApiResponse::created(
            &types::ReposCreateRepoRulesetResponse(rendered),
        ))
    }
}

impl Handler for ops::ReposUpdateRepoRuleset {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let id = ruleset_id(request)?;
        let body: types::ReposUpdateRepoRulesetRequest = request.json_as()?;
        let mut state = lock(state);
        let found = ruleset_repo(&state, request)?;
        let rulesets = &found.repo(&state).setup.rulesets;
        if !rulesets.contains_key(&id) {
            return Err(ApiResponse::not_found());
        }
        if let Some(name) = &body.name
            && rulesets
                .values()
                .any(|ruleset| ruleset.id != id && &ruleset.name == name)
        {
            return Err(duplicate_name());
        }
        let ruleset = found
            .repo_mut(&mut state)
            .setup
            .rulesets
            .get_mut(&id)
            .expect("ruleset found above");
        // Omitted properties keep their values.
        // Source: <https://docs.github.com/en/rest/repos/rules#update-a-repository-ruleset>
        if let Some(name) = body.name {
            ruleset.name = name;
        }
        if let Some(value) = body.target {
            ruleset.target = update_target(value);
        }
        if let Some(enforcement) = body.enforcement {
            ruleset.enforcement = enforcement;
        }
        if let Some(conditions) = body.conditions {
            ruleset.conditions = Some(conditions);
        }
        if let Some(bypass_actors) = body.bypass_actors {
            ruleset.bypass_actors = bypass_actors;
        }
        if let Some(rules) = body.rules {
            ruleset.rules = rules;
        }
        ruleset.updated_at = crate::time::now_utc();
        let ruleset = ruleset.clone();
        Ok(ApiResponse::ok_typed(
            &types::ReposUpdateRepoRulesetResponse(render(request, &found, &ruleset)),
        ))
    }
}
