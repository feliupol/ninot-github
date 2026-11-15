//! The `release` event.

use serde_json::{Map, Value};

use super::{Ctx, PayloadError, model};
use crate::types;
use crate::webhook_types as wt;

/// Queues `release` `action` for `release`.
///
/// `changes` is the edited `name` and `body` (`{"name": {"from": ...}}`). Other actions omit it.
pub(crate) fn deliver(
    ctx: &mut Ctx,
    action: &str,
    release: &types::Release,
    changes: Option<Map<String, Value>>,
) -> Result<(), PayloadError> {
    if !ctx.wants("release") {
        return Ok(());
    }
    let (Some(repository), Some(sender)) = (ctx.repository(), ctx.sender(None)) else {
        return Ok(());
    };
    let mut release = webhook_release(release);
    if action == "unpublished" {
        // An unpublished release is a draft again.
        release.published_at = None;
    }
    macro_rules! payload {
        ($type:ident, $action:ident::$variant:ident) => {
            ctx.enqueue_built(
                "release",
                Ok(wt::$type {
                    action: wt::$action::$variant,
                    installation: None,
                    organization: None,
                    release,
                    repository,
                    sender,
                }),
            )
        };
    }
    match action {
        "created" => payload!(ReleaseCreated, ReleaseCreatedAction::Created),
        "deleted" => payload!(ReleaseDeleted, ReleaseDeletedAction::Deleted),
        "prereleased" => payload!(ReleasePrereleased, ReleasePrereleasedAction::Prereleased),
        "published" => payload!(ReleasePublished, ReleasePublishedAction::Published),
        "released" => payload!(ReleaseReleased, ReleaseReleasedAction::Released),
        "unpublished" => payload!(ReleaseUnpublished, ReleaseUnpublishedAction::Unpublished),
        "edited" => {
            let changes = Value::Object(changes.unwrap_or_default());
            let payload = serde_json::from_value(changes.clone())
                .map_err(|_| PayloadError::Unknown {
                    field: "changes",
                    value: changes.to_string(),
                })
                .map(|changes| wt::ReleaseEdited {
                    action: wt::ReleaseEditedAction::Edited,
                    changes,
                    installation: None,
                    organization: None,
                    release,
                    repository,
                    sender,
                });
            ctx.enqueue_built("release", payload)
        }
        other => ctx.enqueue_built(
            "release",
            Err::<(), _>(PayloadError::Unknown {
                field: "action",
                value: other.to_owned(),
            }),
        ),
    }
}

/// The webhook release for `release`, with an empty name and body when it has none.
fn webhook_release(release: &types::Release) -> wt::Release {
    wt::Release {
        assets: release.assets.iter().map(asset).collect(),
        assets_url: release.assets_url.clone(),
        author: model::user(&release.author),
        body: release.body.clone().unwrap_or_default(),
        created_at: Some(release.created_at),
        discussion_url: release.discussion_url.clone(),
        draft: release.draft,
        html_url: release.html_url.clone(),
        id: release.id,
        mentions_count: release.mentions_count,
        name: release.name.clone().unwrap_or_default(),
        node_id: release.node_id.clone(),
        prerelease: release.prerelease,
        published_at: release.published_at,
        reactions: release
            .reactions
            .as_ref()
            .map(|rollup| model::reactions(Some(rollup), &release.url)),
        tag_name: release.tag_name.clone(),
        tarball_url: release.tarball_url.clone(),
        target_commitish: release.target_commitish.clone(),
        upload_url: release.upload_url.clone(),
        url: release.url.clone(),
        zipball_url: release.zipball_url.clone(),
    }
}

/// The webhook release asset for `asset`. The webhook schema only knows uploaded assets.
fn asset(asset: &types::ReleaseAsset) -> wt::ReleaseAsset {
    wt::ReleaseAsset {
        browser_download_url: asset.browser_download_url.clone(),
        content_type: asset.content_type.clone(),
        created_at: asset.created_at,
        download_count: asset.download_count,
        id: asset.id,
        label: asset.label.clone(),
        name: asset.name.clone(),
        node_id: asset.node_id.clone(),
        size: asset.size,
        state: wt::ReleaseAssetState::Uploaded,
        updated_at: asset.updated_at,
        uploader: asset.uploader.as_ref().map(model::nullable_user),
        url: asset.url.clone(),
    }
}
