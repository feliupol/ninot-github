//! Organizations, memberships and teams over the REST API.

#[path = "../common/mod.rs"]
mod common;
mod team_access;
mod teams;

use common::Api;
use ninot_github::Server;
use serde_json::{Value, json};

const OWNER_SCOPES: &[&str] = &["repo", "admin:org"];

/// A server with the organization `acme` owned by alice and bob, and a user carol outside it.
/// Bob is demoted to a plain member.
pub fn fixture() -> (Server, String, String, String) {
    let server = Server::start().unwrap();
    for user in ["alice", "bob", "carol"] {
        server.add_user(user);
    }
    server.add_org("acme", &["alice", "bob"]);
    let alice = server.add_user("alice").add_token(OWNER_SCOPES);
    let bob = server.add_user("bob").add_token(OWNER_SCOPES);
    let carol = server.add_user("carol").add_token(OWNER_SCOPES);
    let reply = Api::new(&server, Some(&alice))
        .put("/orgs/acme/memberships/bob", json!({"role": "member"}));
    assert_eq!(reply.status, 200, "{}", reply.body);
    (server, alice, bob, carol)
}

pub fn logins(body: &Value) -> Vec<String> {
    body.as_array()
        .unwrap()
        .iter()
        .map(|item| item["login"].as_str().unwrap().to_owned())
        .collect()
}

pub fn slugs(body: &Value) -> Vec<String> {
    body.as_array()
        .unwrap()
        .iter()
        .map(|item| item["slug"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn listing_and_updating_organizations() {
    let (server, alice, bob, _) = fixture();
    server.add_org("zeta", &["alice"]);
    let anon = Api::new(&server, None);
    let alice = Api::new(&server, Some(&alice));
    let bob = Api::new(&server, Some(&bob));

    let all = anon.get("/organizations");
    assert_eq!(all.status, 200);
    assert_eq!(logins(&all.body), ["acme", "zeta"]);
    let first = anon.get("/organizations?per_page=1");
    assert_eq!(logins(&first.body), ["acme"]);
    let next = first.header("link").unwrap();
    assert!(
        next.contains("since=") && next.contains("rel=\"next\""),
        "{next}"
    );
    let since = anon.get(&format!(
        "/organizations?since={}",
        first.body[0]["id"].as_i64().unwrap()
    ));
    assert_eq!(logins(&since.body), ["zeta"]);

    let changed = alice.patch(
        "/orgs/acme",
        json!({"description": "Builders", "company": "Acme Inc", "name": "Acme"}),
    );
    assert_eq!(changed.status, 200, "{}", changed.body);
    assert_eq!(changed.body["description"], "Builders");
    assert_eq!(changed.body["company"], "Acme Inc");
    assert_eq!(anon.get("/orgs/acme").body["name"], "Acme");
    assert_eq!(
        logins(&anon.get("/organizations").body),
        ["acme", "zeta"],
        "listing shows the description"
    );
    assert_eq!(
        anon.get("/organizations").body[0]["description"],
        "Builders"
    );

    assert_eq!(anon.patch("/orgs/acme", json!({})).status, 401);
    assert_eq!(bob.patch("/orgs/acme", json!({})).status, 403);
    assert_eq!(alice.patch("/orgs/missing", json!({})).status, 404);
    assert_eq!(
        alice
            .patch("/orgs/acme", json!({"description": "x".repeat(161)}))
            .status,
        422
    );
    assert_eq!(alice.patch("/orgs/acme", json!({"company": 7})).status, 422);
}

#[test]
fn installations_and_outside_collaborators_are_for_owners() {
    let (server, alice, bob, _) = fixture();
    let alice_api = Api::new(&server, Some(&alice));
    let bob = Api::new(&server, Some(&bob));
    server
        .create_repository("acme", "app", true, None, None, "main")
        .unwrap()
        .create_app_installation(77, 5)
        .unwrap();

    let installs = alice_api.get("/orgs/acme/installations");
    assert_eq!(installs.status, 200, "{}", installs.body);
    assert_eq!(installs.body["total_count"], 1);
    assert_eq!(installs.body["installations"][0]["id"], 77);
    assert_eq!(bob.get("/orgs/acme/installations").status, 403);
    assert_eq!(alice_api.get("/orgs/missing/installations").status, 404);
    assert_eq!(
        Api::new(&server, None)
            .get("/orgs/acme/installations")
            .status,
        401
    );

    let outside = alice_api.get("/orgs/acme/outside_collaborators");
    assert_eq!(outside.status, 200);
    assert_eq!(outside.body, json!([]));
    assert_eq!(bob.get("/orgs/acme/outside_collaborators").status, 403);
    assert_eq!(
        alice_api.get("/orgs/missing/outside_collaborators").status,
        404
    );
    assert_eq!(
        alice_api
            .get("/orgs/acme/outside_collaborators?filter=bogus")
            .status,
        422
    );
}

#[test]
fn members_and_memberships() {
    let (server, alice, bob, carol) = fixture();
    let anon = Api::new(&server, None);
    let alice = Api::new(&server, Some(&alice));
    let bob = Api::new(&server, Some(&bob));
    let carol = Api::new(&server, Some(&carol));

    // Concealed members are invisible to outsiders and anonymous callers.
    assert_eq!(
        logins(&alice.get("/orgs/acme/members").body),
        ["alice", "bob"]
    );
    assert_eq!(
        logins(&alice.get("/orgs/acme/members?role=admin").body),
        ["alice"]
    );
    assert_eq!(
        logins(&alice.get("/orgs/acme/members?role=member").body),
        ["bob"]
    );
    assert_eq!(
        logins(&bob.get("/orgs/acme/members").body),
        ["alice", "bob"]
    );
    assert_eq!(carol.get("/orgs/acme/members").body, json!([]));
    assert_eq!(anon.get("/orgs/acme/members").body, json!([]));
    assert_eq!(anon.get("/orgs/missing/members").status, 404);
    assert_eq!(
        bob.get("/orgs/acme/members?filter=2fa_disabled").status,
        422
    );
    assert_eq!(
        logins(&alice.get("/orgs/acme/members?filter=2fa_disabled").body),
        ["alice", "bob"]
    );
    let paged = alice.get("/orgs/acme/members?per_page=1");
    assert_eq!(logins(&paged.body), ["alice"]);
    assert!(paged.header("link").unwrap().contains("rel=\"next\""));

    // Membership checks.
    assert_eq!(alice.get("/orgs/acme/members/bob").status, 204);
    assert_eq!(alice.get("/orgs/acme/members/carol").status, 404);
    let redirect = carol.get("/orgs/acme/members/bob");
    assert_eq!(redirect.status, 302);
    assert!(
        redirect
            .header("location")
            .unwrap()
            .ends_with("/orgs/acme/public_members/bob")
    );
    assert_eq!(anon.get("/orgs/acme/members/bob").status, 302);
    assert_eq!(alice.get("/orgs/missing/members/bob").status, 404);

    // Reading memberships.
    let own = bob.get("/orgs/acme/memberships/bob");
    assert_eq!(own.status, 200, "{}", own.body);
    assert_eq!(
        (own.body["state"].as_str(), own.body["role"].as_str()),
        (Some("active"), Some("member"))
    );
    assert_eq!(
        alice.get("/orgs/acme/memberships/alice").body["role"],
        "admin"
    );
    assert_eq!(bob.get("/orgs/acme/memberships/alice").status, 200);
    assert_eq!(carol.get("/orgs/acme/memberships/bob").status, 403);
    assert_eq!(alice.get("/orgs/acme/memberships/carol").status, 404);
    assert_eq!(anon.get("/orgs/acme/memberships/bob").status, 401);
    assert_eq!(alice.get("/orgs/missing/memberships/bob").status, 404);

    // Inviting makes a pending membership the invitee can read but not use.
    assert_eq!(
        bob.put("/orgs/acme/memberships/carol", json!({})).status,
        403
    );
    assert_eq!(
        anon.put("/orgs/acme/memberships/carol", json!({})).status,
        401
    );
    assert_eq!(
        alice.put("/orgs/acme/memberships/nobody", json!({})).status,
        404
    );
    assert_eq!(
        alice
            .put("/orgs/missing/memberships/carol", json!({}))
            .status,
        404
    );
    assert_eq!(
        alice
            .put("/orgs/acme/memberships/carol", json!({"role": "owner"}))
            .status,
        422
    );
    let invited = alice.put("/orgs/acme/memberships/carol", json!({"role": "admin"}));
    assert_eq!(invited.status, 200, "{}", invited.body);
    assert_eq!(
        (
            invited.body["state"].as_str(),
            invited.body["role"].as_str()
        ),
        (Some("pending"), Some("admin"))
    );
    assert_eq!(
        carol.get("/orgs/acme/memberships/carol").body["state"],
        "pending"
    );
    assert_eq!(alice.get("/orgs/acme/members/carol").status, 404);
    let promoted = alice.put("/orgs/acme/memberships/bob", json!({"role": "admin"}));
    assert_eq!(promoted.body["role"], "admin");
    assert_eq!(alice.patch("/orgs/acme", json!({})).status, 200);

    // Removing memberships and members.
    assert_eq!(carol.delete("/orgs/acme/memberships/carol").status, 403);
    assert_eq!(alice.delete("/orgs/acme/memberships/carol").status, 204);
    assert_eq!(alice.get("/orgs/acme/memberships/carol").status, 404);
    assert_eq!(alice.delete("/orgs/acme/memberships/carol").status, 404);
    assert_eq!(alice.delete("/orgs/missing/memberships/carol").status, 404);
    assert_eq!(anon.delete("/orgs/acme/members/bob").status, 401);
    assert_eq!(carol.delete("/orgs/acme/members/bob").status, 403);
    assert_eq!(alice.delete("/orgs/acme/members/bob").status, 204);
    assert_eq!(alice.delete("/orgs/acme/members/bob").status, 204);
    assert_eq!(logins(&alice.get("/orgs/acme/members").body), ["alice"]);
    assert_eq!(alice.delete("/orgs/missing/members/bob").status, 404);
}
