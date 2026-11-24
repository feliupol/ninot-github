//! Permissions GitHub App tokens need for each operation, generated from GitHub's GitHub Apps
//! data in `spec/permissions`.
//!
//! Source: <https://docs.github.com/en/rest/authentication/permissions-required-for-github-apps>
//!
//! The data lists every permission an operation accepts, but not whether several of them are
//! alternatives or are all needed: its `additional-permissions` flag covers both.
//! Source: <https://docs.github.com/en/rest/authentication/permissions-required-for-github-apps#about-github-app-permissions>
//! The fake treats them as alternatives, which is right for most of them, such as the issue
//! operations that pull request permissions also allow.

use std::collections::HashMap;

use crate::api::{ApiResponse, Caller};
use crate::operation::OperationInfo;

/// A kind of token the GitHub Apps data describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "the fake mints only installation tokens")
)]
pub enum TokenKind {
    /// A GitHub App installation access token.
    Installation,
    /// A GitHub App user access token.
    UserToServer,
    /// A fine-grained personal access token.
    FineGrainedPat,
}

/// Access level of a permission; each includes the levels before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Access {
    Read,
    Write,
    Admin,
}

impl Access {
    /// The level as the API names it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Write => "write",
            Self::Admin => "admin",
        }
    }
}

/// One permission at one access level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Requirement {
    pub permission: &'static str,
    pub access: Access,
}

include!(concat!(env!("OUT_DIR"), "/permissions.rs"));

/// Refuses an installation token whose grant does not cover `operation`.
///
/// Requests for a repository other than the installation's pass, so the handler answers them.
pub(crate) fn authorize(
    operation: &OperationInfo,
    params: &HashMap<String, String>,
    caller: Option<&Caller>,
) -> Result<(), ApiResponse> {
    let Some(caller) = caller else {
        return Ok(());
    };
    let Some(installation) = caller.installation() else {
        return Ok(());
    };
    if let (Some(owner), Some(repo)) = (params.get("owner"), params.get("repo"))
        && !installation.eq_ignore_ascii_case(&format!("{owner}/{repo}"))
    {
        return Ok(());
    }
    let allowed = requirements(TokenKind::Installation, operation.id).is_some_and(|needed| {
        needed.is_empty()
            || needed.iter().any(|requirement| {
                caller
                    .permission(requirement.permission)
                    .is_some_and(|granted| granted >= requirement.access)
            })
    });
    if allowed {
        Ok(())
    } else {
        // Source: <https://docs.github.com/en/rest/using-the-rest-api/troubleshooting-the-rest-api#resource-not-accessible>
        Err(crate::errors::GithubError::IntegrationForbidden.to_response())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handlers::IMPLEMENTED;

    /// Implemented operations the GitHub Apps data does not list for installation tokens.
    const WITHOUT_INSTALLATION_ACCESS: [&str; 20] = [
        "apps/create-installation-access-token",
        "apps/get-repo-installation",
        "apps/list-installation-repos-for-authenticated-user",
        "apps/list-installations",
        "apps/list-installations-for-authenticated-user",
        "gists/create",
        "gists/list",
        "issues/list",
        "issues/list-for-authenticated-user",
        "issues/list-for-org",
        "repos/list-for-authenticated-user",
        "repos/transfer",
        "repos/upload-release-asset",
        "teams/list-for-authenticated-user",
        "users/create-public-ssh-key-for-authenticated-user",
        "users/follow",
        "users/get-authenticated",
        "users/list-emails-for-authenticated-user",
        "users/list-public-ssh-keys-for-authenticated-user",
        "users/update-authenticated",
    ];

    #[test]
    fn installation_access_of_implemented_operations_matches_the_data() {
        let mut without: Vec<&str> = IMPLEMENTED
            .iter()
            .filter(|info| requirements(TokenKind::Installation, info.id).is_none())
            .map(|info| info.id)
            .collect();
        without.sort_unstable();
        assert_eq!(without, WITHOUT_INSTALLATION_ACCESS);
    }

    #[test]
    fn requirements_come_from_the_data() {
        let check_run = requirements(TokenKind::Installation, "checks/create").unwrap();
        assert_eq!(
            check_run,
            [Requirement {
                permission: "checks",
                access: Access::Write
            }]
        );
        assert_eq!(
            requirements(TokenKind::FineGrainedPat, "checks/create"),
            None
        );
        assert_eq!(
            requirements(TokenKind::Installation, "meta/root"),
            Some(&[][..])
        );
        let comment = requirements(TokenKind::UserToServer, "issues/create-comment").unwrap();
        assert_eq!(
            comment.iter().map(|r| r.permission).collect::<Vec<_>>(),
            ["issues", "pull_requests"]
        );
    }
}
