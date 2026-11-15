//! Team creation, visibility, updates, nesting and deletion.

use crate::common::Api;
use crate::{fixture, slugs};
use serde_json::{Value, json};

pub fn create_team(api: &Api<'_>, body: Value) -> Value {
    let reply = api.post("/orgs/acme/teams", body);
    assert_eq!(reply.status, 201, "{}", reply.body);
    reply.body
}

#[test]
fn teams_are_created_updated_nested_and_deleted() {
    let (server, alice, bob, carol) = fixture();
    let alice = Api::new(&server, Some(&alice));
    let bob = Api::new(&server, Some(&bob));
    let carol = Api::new(&server, Some(&carol));
    let anon = Api::new(&server, None);

    let core = create_team(
        &alice,
        json!({"name": "Core Team", "description": "Builders", "privacy": "closed"}),
    );
    assert_eq!(core["slug"], "core-team");
    assert_eq!(core["privacy"], "closed");
    let hidden = create_team(&alice, json!({"name": "Hidden"}));
    assert_eq!(hidden["privacy"], "secret");
    assert_eq!(core["members_count"], 1);
    assert_eq!(core["organization"]["login"], "acme");
    let open = create_team(&bob, json!({"name": "Open", "privacy": "closed"}));
    let core_id = core["id"].as_i64().unwrap();
    let child = create_team(
        &alice,
        json!({"name": "Child", "parent_team_id": core_id, "permission": "push"}),
    );
    assert_eq!(child["privacy"], "closed");
    assert_eq!(child["parent"]["slug"], "core-team");

    // Visibility: owners see all; members only closed teams and their own secret ones.
    assert_eq!(
        slugs(&alice.get("/orgs/acme/teams").body),
        ["child", "core-team", "hidden", "open"]
    );
    assert_eq!(
        slugs(&bob.get("/orgs/acme/teams").body),
        ["child", "core-team", "open"]
    );
    assert_eq!(carol.get("/orgs/acme/teams").body, json!([]));
    assert_eq!(carol.get("/orgs/acme/teams/open").status, 404);
    assert_eq!(bob.get("/orgs/acme/teams/core-team").status, 200);
    assert_eq!(bob.get("/orgs/acme/teams/hidden").status, 404);
    assert_eq!(bob.get("/orgs/acme/teams/open").body["id"], open["id"]);
    assert_eq!(alice.get("/orgs/acme/teams/Core-Team").status, 200);
    assert_eq!(alice.get("/orgs/acme/teams/missing").status, 404);
    assert_eq!(alice.get("/orgs/missing/teams").status, 404);
    assert_eq!(anon.get("/orgs/acme/teams").status, 401);
    assert_eq!(
        alice.get("/orgs/acme/teams?team_type=enterprise").body,
        json!([])
    );
    let paged = alice.get("/orgs/acme/teams?per_page=2");
    assert_eq!(slugs(&paged.body).len(), 2);
    assert!(paged.header("link").unwrap().contains("rel=\"next\""));
    assert_eq!(
        slugs(&alice.get("/orgs/acme/teams/core-team/teams").body),
        ["child"]
    );
    assert_eq!(alice.get("/orgs/acme/teams/open/teams").body, json!([]));
    assert_eq!(bob.get("/orgs/acme/teams/hidden/teams").status, 404);

    // Create failures.
    assert_eq!(
        carol.post("/orgs/acme/teams", json!({"name": "x"})).status,
        403
    );
    assert_eq!(
        anon.post("/orgs/acme/teams", json!({"name": "x"})).status,
        401
    );
    assert_eq!(
        alice
            .post("/orgs/missing/teams", json!({"name": "x"}))
            .status,
        404
    );
    for body in [
        json!({"name": "core team"}),
        json!({"name": "a", "parent_team_id": hidden["id"]}),
        json!({"name": "  "}),
        json!({"name": "a", "parent_team_id": 999_999}),
        json!({"name": "a", "parent_team_id": core_id, "privacy": "secret"}),
        json!({"name": "a", "maintainers": ["carol"]}),
        json!({"name": "a", "repo_names": ["acme/none"]}),
        json!({"name": "a", "privacy": "public"}),
    ] {
        assert_eq!(
            alice.post("/orgs/acme/teams", body.clone()).status,
            422,
            "{body}"
        );
    }

    // Updates: owners and maintainers manage; renaming re-slugs; privacy and parent rules hold.
    assert_eq!(bob.patch("/orgs/acme/teams/open", json!({})).status, 200);
    assert_eq!(bob.patch("/orgs/acme/teams/hidden", json!({})).status, 404);
    assert_eq!(
        bob.patch("/orgs/acme/teams/core-team", json!({})).status,
        403
    );
    let renamed = alice.patch(
        "/orgs/acme/teams/core-team",
        json!({"name": "Platform", "description": "Platform folks", "notification_setting": "notifications_disabled"}),
    );
    assert_eq!(renamed.status, 200, "{}", renamed.body);
    assert_eq!(renamed.body["slug"], "platform");
    assert_eq!(
        renamed.body["notification_setting"],
        "notifications_disabled"
    );
    assert_eq!(alice.get("/orgs/acme/teams/core-team").status, 404);
    assert_eq!(
        alice.get("/orgs/acme/teams/child").body["parent"]["slug"],
        "platform"
    );
    assert_eq!(
        alice
            .patch("/orgs/acme/teams/platform", json!({"name": "open"}))
            .status,
        422
    );
    assert_eq!(
        alice
            .patch("/orgs/acme/teams/platform", json!({"privacy": "secret"}))
            .status,
        422,
        "a team with children cannot be secret"
    );
    assert_eq!(
        alice
            .patch(
                "/orgs/acme/teams/platform",
                json!({"parent_team_id": child["id"]})
            )
            .status,
        422,
        "a team cannot sit below its own child"
    );
    let moved = alice.patch("/orgs/acme/teams/child", json!({"parent_team_id": null}));
    assert_eq!(moved.body["parent"], Value::Null);
    let regrouped = alice.patch(
        "/orgs/acme/teams/child",
        json!({"parent_team_id": open["id"]}),
    );
    assert_eq!(regrouped.body["parent"]["slug"], "open");
    assert_eq!(
        alice.patch("/orgs/missing/teams/open", json!({})).status,
        404
    );
    assert_eq!(anon.patch("/orgs/acme/teams/open", json!({})).status, 401);

    // A member who is not a maintainer cannot manage the team.
    assert_eq!(
        alice
            .put(
                "/orgs/acme/teams/open/memberships/bob",
                json!({"role": "member"})
            )
            .status,
        200
    );
    assert_eq!(bob.patch("/orgs/acme/teams/open", json!({})).status, 403);
    assert_eq!(bob.delete("/orgs/acme/teams/open").status, 403);

    // Deleting a parent deletes its descendants.
    assert_eq!(alice.delete("/orgs/acme/teams/open").status, 204);
    assert_eq!(alice.get("/orgs/acme/teams/child").status, 404);
    assert_eq!(alice.delete("/orgs/acme/teams/open").status, 404);
    assert_eq!(carol.delete("/orgs/acme/teams/platform").status, 404);
    assert_eq!(anon.delete("/orgs/acme/teams/platform").status, 401);
    assert_eq!(alice.delete("/orgs/missing/teams/platform").status, 404);
    assert_eq!(alice.delete("/orgs/acme/teams/platform").status, 204);
}
