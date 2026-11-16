//! The authenticated user's profile, emails, keys and follows, and the user directory.

mod common;

use common::Api;
use ninot_github::Server;
use serde_json::json;

const KEY: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIOMqqnkVzrm0SdG6UOoqKLsabgH5C9okWi0dh2l9GKJl";

fn user<'a>(server: &'a Server, login: &str, scopes: &[&str]) -> Api<'a> {
    let user = server.add_user(login);
    Api::new(server, Some(&user.add_token(scopes)))
}

#[test]
fn the_authenticated_user_updates_their_profile() {
    let server = Server::start().unwrap();
    let alice = user(&server, "alice", &["user"]);
    let updated = alice.patch(
        "/user",
        json!({ "name": "Alice", "bio": "Bots", "location": "Mars", "hireable": true, "twitter_username": null }),
    );
    assert_eq!(updated.status, 200, "{}", updated.body);
    assert_eq!(updated.body["login"], "alice");
    assert_eq!(updated.body["name"], "Alice");
    assert_eq!(updated.body["bio"], "Bots");
    assert_eq!(updated.body["hireable"], true);
    let profile = Api::new(&server, None).get("/users/alice");
    assert_eq!(profile.body["location"], "Mars", "{}", profile.body);

    let emails = alice.get("/user/emails");
    assert_eq!(emails.status, 200, "{}", emails.body);
    let email = emails.body[0]["email"].as_str().unwrap().to_owned();
    assert_eq!(emails.body[0]["primary"], true);
    let public = alice.patch("/user", json!({ "email": email }));
    assert_eq!(public.status, 200, "{}", public.body);
    assert_eq!(public.body["email"], email.as_str());
    assert_eq!(alice.get("/user/emails").body[0]["visibility"], "public");
    let unverified = alice.patch("/user", json!({ "email": "someone@example.com" }));
    assert_eq!(unverified.status, 422, "{}", unverified.body);

    // Changing the profile needs the `user` scope.
    let repo_only = user(&server, "bob", &["repo"]);
    assert_eq!(repo_only.patch("/user", json!({ "bio": "x" })).status, 403);
    assert_eq!(repo_only.get("/user/emails").status, 403);
    assert_eq!(
        Api::new(&server, None).patch("/user", json!({})).status,
        401
    );
    assert_eq!(Api::new(&server, None).get("/user/emails").status, 401);
}

#[test]
fn public_ssh_keys_are_added_and_listed() {
    let server = Server::start().unwrap();
    let alice = user(&server, "alice", &["write:public_key"]);
    let created = alice.post(
        "/user/keys",
        json!({ "title": "laptop", "key": format!("{KEY} alice@laptop") }),
    );
    assert_eq!(created.status, 201, "{}", created.body);
    assert_eq!(created.body["key"], KEY);
    assert_eq!(created.body["title"], "laptop");
    let again = alice.post("/user/keys", json!({ "key": KEY }));
    assert_eq!(again.status, 422, "{}", again.body);
    let invalid = alice.post("/user/keys", json!({ "key": "not a key" }));
    assert_eq!(invalid.status, 422, "{}", invalid.body);

    let own = alice.get("/user/keys");
    assert_eq!(own.status, 200, "{}", own.body);
    assert_eq!(own.body[0]["id"], created.body["id"]);
    let public = Api::new(&server, None).get("/users/alice/keys");
    assert_eq!(public.status, 200, "{}", public.body);
    assert_eq!(public.body[0]["key"], KEY);
    assert_eq!(
        Api::new(&server, None).get("/users/nobody/keys").status,
        404
    );
    assert_eq!(Api::new(&server, None).get("/user/keys").status, 401);
    let unscoped = user(&server, "bob", &["repo"]);
    assert_eq!(unscoped.get("/user/keys").status, 403);
}

#[test]
fn followers_are_listed() {
    let server = Server::start().unwrap();
    server.add_user("alice");
    let bob = user(&server, "bob", &["user:follow"]);
    assert_eq!(bob.put("/user/following/alice", json!({})).status, 204);
    assert_eq!(bob.put("/user/following/nobody", json!({})).status, 404);
    let followers = Api::new(&server, None).get("/users/alice/followers");
    assert_eq!(followers.status, 200, "{}", followers.body);
    assert_eq!(followers.body[0]["login"], "bob");
    assert_eq!(
        Api::new(&server, None).get("/users/alice").body["followers"],
        1
    );
    assert_eq!(
        Api::new(&server, None)
            .get("/users/nobody/followers")
            .status,
        404
    );
}

#[test]
fn users_are_found_by_id_and_listed_since_an_id() {
    let server = Server::start().unwrap();
    server.add_user("alice");
    server.add_org("acme", &["alice"]);
    let anonymous = Api::new(&server, None);
    let alice = anonymous.get("/users/alice");
    let by_id = anonymous.get(&format!("/user/{}", alice.body["id"]));
    assert_eq!(by_id.status, 200, "{}", by_id.body);
    assert_eq!(by_id.body["login"], "alice");
    assert_eq!(anonymous.get("/user/999999").status, 404);
    // Literal `/user/...` routes still win over the id route.
    assert_eq!(Api::admin(&server).get("/user/repos").status, 200);

    let first = anonymous.get("/users?per_page=1");
    assert_eq!(first.status, 200, "{}", first.body);
    assert_eq!(first.body.as_array().unwrap().len(), 1);
    let link = first.header("link").unwrap().to_owned();
    let since = first.body[0]["id"].as_i64().unwrap();
    assert!(link.contains(&format!("since={since}")), "{link}");
    let rest = anonymous.get(&format!("/users?since={since}"));
    let logins: Vec<&str> = rest
        .body
        .as_array()
        .unwrap()
        .iter()
        .map(|u| u["login"].as_str().unwrap())
        .collect();
    assert!(
        logins.contains(&"acme") && logins.contains(&"alice"),
        "{logins:?}"
    );
    assert!(rest.header("link").is_none());
}
