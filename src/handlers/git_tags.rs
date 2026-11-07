//! Annotated git tag objects: create and read tags in the hosted object database.

use std::sync::Mutex;

use super::Handler;
use super::git_store::{
    Person, find_existing_object, internal, locate, locate_writable, object_id, object_node_id,
    open_repo, unsigned_verification,
};
use crate::api::{ApiRequest, ApiResponse, ApiResult};
use crate::operation::ops;
use crate::render;
use crate::state::{State, lock};
use crate::types;

fn tag_url(request: &ApiRequest, located: &super::git_store::Located, sha: &str) -> String {
    format!("{}/git/tags/{sha}", located.api(request))
}

fn object_collection(kind: gix::objs::Kind) -> &'static str {
    match kind {
        gix::objs::Kind::Commit => "commits",
        gix::objs::Kind::Tree => "trees",
        gix::objs::Kind::Blob => "blobs",
        gix::objs::Kind::Tag => "tags",
    }
}

fn render_tag(
    request: &ApiRequest,
    located: &super::git_store::Located,
    sha: &str,
    tag: gix::objs::TagRef<'_>,
) -> Result<types::GitTag, ApiResponse> {
    let api = located.api(request);
    let target = tag.target();
    let tagger = tag
        .tagger()
        .map_err(internal)?
        .ok_or_else(|| internal("tag has no tagger"))?;
    let time = tagger.time().map_err(internal)?;
    let date = render::timestamp(&time.format_or_unix(gix::date::time::format::ISO8601_STRICT));
    Ok(types::GitTag {
        message: tag.message.to_string(),
        node_id: object_node_id("Tag", sha),
        object: types::GitTagObject {
            sha: target.to_string(),
            type_: tag.target_kind.to_string(),
            url: format!("{api}/git/{}/{target}", object_collection(tag.target_kind)),
        },
        sha: sha.to_owned(),
        tag: tag.name.to_string(),
        tagger: types::GitTagTagger {
            date: date.to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            email: tagger.email.to_string(),
            name: tagger.name.to_string(),
        },
        url: tag_url(request, located, sha),
        verification: Some(unsigned_verification()),
    })
}

impl Handler for ops::GitCreateTag {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let (caller, located) = locate_writable(state, request)?;
        let body: types::GitCreateTagRequest = request.json_as()?;
        let repo = open_repo(&located.git_dir)?;
        let target = object_id(&body.object)?;
        let header = repo
            .find_header(target)
            .map_err(|_| ApiResponse::error(422, "object does not exist"))?;
        let expected = match body.type_ {
            types::GitCreateTagRequestType::Commit => gix::objs::Kind::Commit,
            types::GitCreateTagRequestType::Tree => gix::objs::Kind::Tree,
            types::GitCreateTagRequestType::Blob => gix::objs::Kind::Blob,
        };
        if header.kind() != expected {
            return Err(ApiResponse::error(
                422,
                "object type does not match the provided type",
            ));
        }
        let now = render::timestamp(&crate::time::now());
        let state = lock(state);
        let account = state
            .account(&caller.login)
            .expect("authenticated callers have accounts");
        let default = Person::from_account(account, now);
        drop(state);
        let tagger = match &body.tagger {
            Some(tagger) => {
                Person::named(&tagger.name, &tagger.email, tagger.date, now)?.signature()
            }
            None => default.signature(),
        };
        let tag = gix::objs::Tag {
            target,
            target_kind: expected,
            name: body.tag.as_str().into(),
            tagger: Some(tagger),
            message: body.message.as_str().into(),
            signature: None,
        };
        let sha = repo
            .write_object(&tag)
            .map_err(internal)?
            .detach()
            .to_string();
        let stored = repo
            .find_tag(gix::ObjectId::from_hex(sha.as_bytes()).map_err(internal)?)
            .map_err(internal)?;
        let decoded = stored.decode().map_err(internal)?;
        let url = tag_url(request, &located, &sha);
        Ok(ApiResponse::typed(
            201,
            &types::GitCreateTagResponse(render_tag(request, &located, &sha, decoded)?),
        )
        .with_header("location", url))
    }
}

impl Handler for ops::GitGetTag {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let located = locate(state, request)?;
        let tag_sha = request.param("tag_sha");
        let id = find_existing_object(&located.git_dir, tag_sha)?;
        let repo = open_repo(&located.git_dir)?;
        let header = repo.find_header(id).map_err(|_| ApiResponse::not_found())?;
        if header.kind() != gix::objs::Kind::Tag {
            return Err(ApiResponse::not_found());
        }
        let stored = repo.find_tag(id).map_err(internal)?;
        let decoded = stored.decode().map_err(internal)?;
        Ok(ApiResponse::typed(
            200,
            &types::GitGetTagResponse(render_tag(request, &located, &id.to_string(), decoded)?),
        ))
    }
}
