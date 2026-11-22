//! The authenticated user's profile, emails, SSH keys and follows, and the user directory.
//!
//! Source: <https://docs.github.com/en/rest/users>

use std::sync::Mutex;

use super::Handler;
use crate::api::{ApiRequest, ApiResponse, ApiResult, PER_PAGE_DEFAULT, PER_PAGE_MAX, paginate};
use crate::operation::ops;
use crate::render;
use crate::state::{Account, SshKey, State, lock};
use crate::types;

/// The address GitHub gives every user to commit with, which is always verified and primary
/// in this fake.
/// Source: <https://docs.github.com/en/account-and-profile/setting-up-and-managing-your-personal-account-on-github/managing-email-preferences/setting-your-commit-email-address#about-no-reply-email-addresses>
pub(crate) fn noreply_email(account: &Account) -> String {
    format!("{}+{}@users.noreply.github.com", account.id, account.login)
}

/// The verified addresses of `account`; its public email is one of them.
fn emails(account: &Account) -> Vec<types::Email> {
    let email = noreply_email(account);
    let public = account.user_profile.email.as_deref() == Some(email.as_str());
    vec![types::Email {
        email,
        primary: true,
        verified: true,
        visibility: Some(if public { "public" } else { "private" }.to_owned()),
    }]
}

fn caller_account<'a>(state: &'a State, login: &str) -> Result<&'a Account, ApiResponse> {
    state.account(login).ok_or_else(ApiResponse::not_found)
}

/// `account` as `GET /user` shows it to the account itself.
fn private_user(request: &ApiRequest, account: &Account) -> types::PrivateUser {
    let public = render::public_user(request, account);
    let private_repos = account.repos().filter(|repo| repo.private).count() as i64;
    types::PrivateUser {
        avatar_url: public.avatar_url,
        bio: public.bio,
        blog: public.blog,
        business_plus: None,
        collaborators: 0,
        company: public.company,
        created_at: public.created_at,
        disk_usage: 0,
        email: public.email,
        events_url: public.events_url,
        followers: public.followers,
        followers_url: public.followers_url,
        following: public.following,
        following_url: public.following_url,
        gists_url: public.gists_url,
        gravatar_id: public.gravatar_id,
        hireable: public.hireable,
        html_url: public.html_url,
        id: public.id,
        ldap_dn: None,
        location: public.location,
        login: public.login,
        name: public.name,
        node_id: public.node_id,
        notification_email: None,
        organizations_url: public.organizations_url,
        owned_private_repos: private_repos,
        plan: None,
        private_gists: 0,
        public_gists: public.public_gists,
        public_repos: public.public_repos,
        received_events_url: public.received_events_url,
        repos_url: public.repos_url,
        site_admin: false,
        starred_url: public.starred_url,
        subscriptions_url: public.subscriptions_url,
        total_private_repos: private_repos,
        twitter_username: public.twitter_username,
        two_factor_authentication: false,
        type_: public.type_,
        updated_at: public.updated_at,
        url: public.url,
        user_view_type: Some("private".to_owned()),
    }
}

impl Handler for ops::UsersUpdateAuthenticated {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let login = request.require_scope(&["user"])?.login.clone();
        let body: types::UsersUpdateAuthenticatedRequest = request.json_as()?;
        let mut state = lock(state);
        let account = state
            .account_mut(&login)
            .ok_or_else(ApiResponse::not_found)?;
        // The public email must be one of the user's verified addresses; empty hides it.
        // Unsourced for the API; the profile settings page offers only verified addresses.
        let email = match body.email.as_deref() {
            None => account.user_profile.email.clone(),
            Some("") => None,
            Some(email) if emails(account).iter().any(|known| known.email == email) => {
                Some(email.to_owned())
            }
            Some(_) => return Err(ApiResponse::validation_failed("User", "email", "invalid")),
        };
        let profile = &mut account.user_profile;
        profile.email = email;
        for (field, value) in [
            (&mut profile.bio, body.bio),
            (&mut profile.blog, body.blog),
            (&mut profile.company, body.company),
            (&mut profile.location, body.location),
        ] {
            if value.is_some() {
                *field = value;
            }
        }
        if let Some(hireable) = body.hireable {
            profile.hireable = Some(hireable);
        }
        if let Some(twitter) = body.twitter_username {
            profile.twitter_username = twitter;
        }
        if let Some(name) = body.name {
            account.name = Some(name);
        }
        Ok(ApiResponse::ok_typed(&private_user(request, account)))
    }
}

impl Handler for ops::UsersListEmailsForAuthenticatedUser {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let login = request
            .require_scope(&["user", "user:email"])?
            .login
            .clone();
        let state = lock(state);
        let account = caller_account(&state, &login)?;
        Ok(paginate(request, &request.path, emails(account)))
    }
}

fn key_url(request: &ApiRequest, key: &SshKey) -> String {
    request.urls().api(&format!("/user/keys/{}", key.id))
}

fn full_key(request: &ApiRequest, key: &SshKey) -> types::Key {
    types::Key {
        created_at: key.created_at,
        id: render::id(key.id),
        key: key.key.clone(),
        last_used: None,
        read_only: false,
        title: key.title.clone(),
        url: key_url(request, key),
        verified: true,
    }
}

impl Handler for ops::UsersListPublicSshKeysForAuthenticatedUser {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let login = request
            .require_scope(&["read:public_key", "write:public_key", "admin:public_key"])?
            .login
            .clone();
        let state = lock(state);
        let account = caller_account(&state, &login)?;
        let keys = account
            .ssh_keys
            .iter()
            .map(|key| full_key(request, key))
            .collect();
        Ok(paginate(request, &request.path, keys))
    }
}

impl Handler for ops::UsersCreatePublicSshKeyForAuthenticatedUser {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let login = request
            .require_scope(&["write:public_key", "admin:public_key"])?
            .login
            .clone();
        let body: types::UsersCreatePublicSshKeyForAuthenticatedUserRequest = request.json_as()?;
        // GitHub stores the type and key material and drops the comment.
        let key = body
            .key
            .split_whitespace()
            .take(2)
            .collect::<Vec<_>>()
            .join(" ");
        let mut state = lock(state);
        if state
            .accounts()
            .any(|account| account.ssh_keys.iter().any(|known| known.key == key))
        {
            // Source: <https://docs.github.com/en/authentication/troubleshooting-ssh/error-key-already-in-use>
            return Err(ApiResponse::validation_failed("PublicKey", "key", "custom"));
        }
        let id = state.next_id();
        let account = state
            .account_mut(&login)
            .ok_or_else(ApiResponse::not_found)?;
        let stored = SshKey {
            id,
            title: body.title.unwrap_or_default(),
            key,
            created_at: crate::time::now_utc(),
        };
        let response = full_key(request, &stored);
        account.ssh_keys.push(stored);
        Ok(ApiResponse::created(&response))
    }
}

impl Handler for ops::UsersListPublicKeysForUser {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let account = state
            .account(request.param("username"))
            .ok_or_else(ApiResponse::not_found)?;
        let keys = account
            .ssh_keys
            .iter()
            .map(|key| types::KeySimple {
                created_at: Some(key.created_at),
                id: render::id(key.id),
                key: key.key.clone(),
                last_used: None,
            })
            .collect();
        Ok(paginate(request, &request.path, keys))
    }
}

impl Handler for ops::UsersFollow {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let login = request
            .require_scope(&["user", "user:follow"])?
            .login
            .clone();
        let mut state = lock(state);
        let followed = state
            .account_mut(request.param("username"))
            .ok_or_else(ApiResponse::not_found)?;
        followed.followers.insert(login.to_lowercase());
        Ok(ApiResponse::no_content())
    }
}

impl Handler for ops::UsersListFollowersForUser {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let state = lock(state);
        let account = state
            .account(request.param("username"))
            .ok_or_else(ApiResponse::not_found)?;
        let followers = account
            .followers
            .iter()
            .filter_map(|login| state.account(login))
            .map(|follower| render::simple_user(request, follower))
            .collect();
        Ok(paginate(request, &request.path, followers))
    }
}

impl Handler for ops::UsersGetById {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let id: u64 = request
            .param("account_id")
            .parse()
            .map_err(|_| ApiResponse::not_found())?;
        let state = lock(state);
        let account = state
            .accounts()
            .find(|account| account.id == id)
            .ok_or_else(ApiResponse::not_found)?;
        Ok(ApiResponse::ok_typed(&render::public_user(
            request, account,
        )))
    }
}

/// Every user and organization in signup order, paged by the last id seen rather than by page.
/// Source: <https://docs.github.com/en/rest/users/users#list-users>
impl Handler for ops::UsersList {
    fn handle(state: &Mutex<State>, request: &ApiRequest) -> ApiResult {
        let since: u64 = match request.query("since") {
            None => 0,
            Some(value) => value
                .parse()
                .map_err(|_| ApiResponse::validation_failed("User", "since", "invalid"))?,
        };
        let per_page = request
            .query("per_page")
            .and_then(|value| value.parse::<usize>().ok())
            .unwrap_or(PER_PAGE_DEFAULT)
            .clamp(1, PER_PAGE_MAX);
        let state = lock(state);
        let mut accounts: Vec<&Account> = state
            .accounts()
            .filter(|account| account.id > since)
            .collect();
        accounts.sort_by_key(|account| account.id);
        let more = accounts.len() > per_page;
        accounts.truncate(per_page);
        let users: Vec<types::SimpleUser> = accounts
            .iter()
            .map(|account| render::simple_user(request, account))
            .collect();
        let response = ApiResponse::ok_typed(&users);
        Ok(match accounts.last() {
            Some(last) if more => {
                let url = request
                    .urls()
                    .api(&format!("/users?per_page={per_page}&since={}", last.id));
                response.with_header("link", format!("<{url}>; rel=\"next\""))
            }
            _ => response,
        })
    }
}
