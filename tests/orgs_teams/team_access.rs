//! Team memberships and the repository access teams grant.

use crate::common::Api;
use crate::teams::create_team;
use crate::{fixture, logins, slugs};
use serde_json::{Value, json};

#[test]
fn team_memberships() {
    let (server, alice, bob, carol) = fixture();
    let alice = Api::new(&server, Some(&alice));
    let bob = Api::new(&server, Some(&bob));
    let carol = Api::new(&server, Some(&carol));
    let anon = Api::new(&server, None);
    create_team(&alice, json!({"name": "Dev", "privacy": "closed"}));
    let team = "/orgs/acme/teams/dev";

    // The creator maintains the team.
    assert_eq!(
        logins(&alice.get(&format!("{team}/members")).body),
        ["alice"]
    );
    let creator = alice.get(&format!("{team}/memberships/alice"));
    assert_eq!(
        (
            creator.body["role"].as_str(),
            creator.body["state"].as_str()
        ),
        (Some("maintainer"), Some("active"))
    );

    let added = alice.put(&format!("{team}/memberships/bob"), json!({}));
    assert_eq!(added.status, 200, "{}", added.body);
    assert_eq!(
        (added.body["role"].as_str(), added.body["state"].as_str()),
        (Some("member"), Some("active"))
    );
    assert_eq!(
        logins(&alice.get(&format!("{team}/members?role=member")).body),
        ["bob"]
    );
    assert_eq!(
        logins(&alice.get(&format!("{team}/members?role=maintainer")).body),
        ["alice"]
    );
    assert_eq!(
        logins(&bob.get(&format!("{team}/members")).body),
        ["alice", "bob"]
    );
    let raised = alice.put(
        &format!("{team}/memberships/bob"),
        json!({"role": "maintainer"}),
    );
    assert_eq!(raised.body["role"], "maintainer");
    assert_eq!(
        bob.get(&format!("{team}/memberships/bob")).body["role"],
        "maintainer"
    );
    let paged = alice.get(&format!("{team}/members?per_page=1"));
    assert!(paged.header("link").unwrap().contains("rel=\"next\""));

    // Someone outside the organization is invited and stays pending.
    let invited = alice.put(&format!("{team}/memberships/carol"), json!({}));
    assert_eq!(invited.body["state"], "pending");
    assert_eq!(
        alice.get(&format!("{team}/memberships/carol")).body["state"],
        "pending"
    );
    assert_eq!(
        alice.get("/orgs/acme/memberships/carol").body["state"],
        "pending"
    );
    assert_eq!(
        logins(&alice.get(&format!("{team}/members")).body),
        ["alice", "bob"]
    );

    // Failures.
    assert_eq!(carol.get(&format!("{team}/members")).status, 404);
    assert_eq!(anon.get(&format!("{team}/members")).status, 401);
    assert_eq!(alice.get("/orgs/acme/teams/none/members").status, 404);
    assert_eq!(alice.get(&format!("{team}/memberships/nobody")).status, 404);
    assert_eq!(
        carol
            .put(&format!("{team}/memberships/carol"), json!({}))
            .status,
        404
    );
    assert_eq!(
        alice
            .put(&format!("{team}/memberships/nobody"), json!({}))
            .status,
        404
    );
    assert_eq!(
        alice
            .put(&format!("{team}/memberships/bob"), json!({"role": "boss"}))
            .status,
        422
    );
    assert_eq!(
        anon.put(&format!("{team}/memberships/bob"), json!({}))
            .status,
        401
    );
    assert_eq!(
        alice.delete("/orgs/acme/teams/none/memberships/bob").status,
        404
    );

    // A member who does not maintain the team cannot change it.
    assert_eq!(
        alice
            .put(
                &format!("{team}/memberships/bob"),
                json!({"role": "member"})
            )
            .status,
        200
    );
    assert_eq!(
        bob.put(&format!("{team}/memberships/bob"), json!({}))
            .status,
        403
    );
    assert_eq!(bob.delete(&format!("{team}/memberships/alice")).status, 403);

    // Removal.
    assert_eq!(alice.delete(&format!("{team}/memberships/bob")).status, 204);
    assert_eq!(
        alice.delete(&format!("{team}/memberships/carol")).status,
        204
    );
    assert_eq!(alice.get(&format!("{team}/memberships/bob")).status, 404);
    assert_eq!(alice.get(&format!("{team}/memberships/carol")).status, 404);

    // The authenticated user's teams.
    assert_eq!(slugs(&alice.get("/user/teams").body), ["dev"]);
    assert_eq!(
        alice.get("/user/teams").body[0]["organization"]["login"],
        "acme"
    );
    assert_eq!(bob.get("/user/teams").body, json!([]));
    assert_eq!(anon.get("/user/teams").status, 401);
    let limited = Api::new(
        &server,
        Some(&server.add_user("alice").add_token(&["repo"])),
    );
    assert_eq!(limited.get("/user/teams").status, 403);
}

#[test]
fn team_repositories_grant_access() {
    let (server, alice, bob, carol) = fixture();
    let alice = Api::new(&server, Some(&alice));
    let bob = Api::new(&server, Some(&bob));
    let carol = Api::new(&server, Some(&carol));
    let anon = Api::new(&server, None);
    for name in ["app", "docs", "misc"] {
        let made = alice.post("/orgs/acme/repos", json!({"name": name, "private": true}));
        assert_eq!(made.status, 201, "{}", made.body);
    }
    server.add_user("dave");
    create_team(&alice, json!({"name": "Dev", "privacy": "closed"}));
    assert_eq!(
        alice
            .put("/orgs/acme/teams/dev/memberships/bob", json!({}))
            .status,
        200
    );
    let team = "/orgs/acme/teams/dev";

    assert_eq!(alice.get(&format!("{team}/repos")).body, json!([]));
    assert_eq!(alice.get(&format!("{team}/repos/acme/app")).status, 404);
    assert_eq!(
        alice
            .put(
                &format!("{team}/repos/acme/app"),
                json!({"permission": "push"})
            )
            .status,
        204
    );
    assert_eq!(
        alice
            .put(&format!("{team}/repos/acme/docs"), json!({}))
            .status,
        204
    );
    let listed = alice.get(&format!("{team}/repos"));
    assert_eq!(listed.status, 200, "{}", listed.body);
    let names: Vec<_> = listed
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|repo| repo["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["app", "docs"]);
    assert_eq!(
        alice
            .get(&format!("{team}/repos?per_page=1"))
            .body
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(alice.get(&format!("{team}/repos/acme/app")).status, 204);

    let accept = |api: &Api<'_>| {
        let client = reqwest::blocking::Client::new();
        let response = client
            .get(format!("{}{team}/repos/acme/app", api.server.url()))
            .header(
                "Authorization",
                format!("token {}", api.token.as_ref().unwrap()),
            )
            .header("Accept", "application/vnd.github.v3.repository+json")
            .send()
            .unwrap();
        (
            response.status().as_u16(),
            response.json::<Value>().unwrap(),
        )
    };
    let (status, repo) = accept(&alice);
    assert_eq!(status, 200, "{repo}");
    assert_eq!(repo["role_name"], "push");
    assert_eq!(repo["permissions"]["push"], true);
    assert_eq!(repo["permissions"]["admin"], false);

    // The grant reaches the team's members: push on app, and the default read on misc.
    let seen = bob.get("/repos/acme/app");
    assert_eq!(seen.status, 200);
    assert_eq!(seen.body["permissions"]["push"], true);
    assert_eq!(
        bob.get("/repos/acme/docs").body["permissions"]["push"],
        false
    );
    assert_eq!(carol.get("/repos/acme/app").status, 404);

    // With no default access, only the team grant remains.
    assert_eq!(
        alice
            .patch(
                "/orgs/acme",
                json!({"default_repository_permission": "none"})
            )
            .status,
        200
    );
    assert_eq!(bob.get("/repos/acme/app").status, 200);
    assert_eq!(bob.get("/repos/acme/misc").status, 404);

    // Failures.
    assert_eq!(
        alice
            .put(
                &format!("{team}/repos/acme/app"),
                json!({"permission": "owner"})
            )
            .status,
        422
    );
    assert_eq!(
        alice
            .put(&format!("{team}/repos/acme/none"), json!({}))
            .status,
        404
    );
    assert_eq!(
        alice
            .put(&format!("{team}/repos/alice/app"), json!({}))
            .status,
        404
    );
    assert_eq!(
        bob.put(&format!("{team}/repos/acme/app"), json!({})).status,
        403
    );
    assert_eq!(
        carol
            .put(&format!("{team}/repos/acme/app"), json!({}))
            .status,
        404
    );
    assert_eq!(
        anon.put(&format!("{team}/repos/acme/app"), json!({}))
            .status,
        401
    );
    assert_eq!(
        alice
            .put("/orgs/acme/teams/none/repos/acme/app", json!({}))
            .status,
        404
    );
    assert_eq!(bob.get(&format!("{team}/repos/acme/none")).status, 404);
    assert_eq!(alice.get(&format!("{team}/repos/nobody/app")).status, 404);
    assert_eq!(anon.get(&format!("{team}/repos")).status, 401);
    assert_eq!(carol.get(&format!("{team}/repos")).status, 404);

    // Child teams inherit the parent's grants.
    let parent = alice.get(team).body["id"].clone();
    create_team(
        &alice,
        json!({"name": "Sub", "privacy": "closed", "parent_team_id": parent}),
    );
    assert_eq!(
        alice
            .put("/orgs/acme/teams/sub/memberships/bob", json!({}))
            .status,
        200
    );
    assert_eq!(alice.delete(&format!("{team}/memberships/bob")).status, 204);
    assert_eq!(bob.get("/repos/acme/app").status, 200);

    // Removal.
    assert_eq!(bob.delete(&format!("{team}/repos/acme/app")).status, 403);
    assert_eq!(alice.delete(&format!("{team}/repos/acme/app")).status, 204);
    assert_eq!(alice.delete(&format!("{team}/repos/acme/app")).status, 204);
    assert_eq!(alice.get(&format!("{team}/repos/acme/app")).status, 404);
    assert_eq!(bob.get("/repos/acme/app").status, 404);
    assert_eq!(alice.delete(&format!("{team}/repos/acme/none")).status, 404);
    assert_eq!(anon.delete(&format!("{team}/repos/acme/docs")).status, 401);
    assert_eq!(
        alice
            .get(&format!("{team}/repos"))
            .body
            .as_array()
            .unwrap()
            .len(),
        1
    );
}
