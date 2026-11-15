//! HTTP locations of GitHub resources, shaped as GitHub shapes them.
//!
//! [`Urls`] holds the API and web base URLs a client addressed; its methods return one
//! resource's locations. `api` URLs sit under the API base, `html` URLs under the web base.

use crate::render::web_host;

/// API and web base URLs, such as `http://host/api/v3` and `http://host`.
#[derive(Clone, Copy)]
pub(crate) struct Urls<'a> {
    api: &'a str,
    web: &'a str,
}

impl<'a> Urls<'a> {
    pub(crate) fn new(api: &'a str, web: &'a str) -> Self {
        Self { api, web }
    }

    /// An endpoint under the API base, such as `/user` or a URI template.
    pub(crate) fn api(&self, path: &str) -> String {
        format!("{}{path}", self.api)
    }

    /// A page under the web base, such as `/settings`.
    pub(crate) fn web(&self, path: &str) -> String {
        format!("{}{path}", self.web)
    }

    pub(crate) fn avatar(&self, login: &str) -> String {
        format!("{}/avatars/{login}", self.web)
    }

    pub(crate) fn user(&self, login: &str) -> AccountUrls {
        AccountUrls {
            api: format!("{}/users/{login}", self.api),
            html: format!("{}/{login}", self.web),
            avatar: self.avatar(login),
        }
    }

    pub(crate) fn org(&self, login: &str) -> AccountUrls {
        AccountUrls {
            api: format!("{}/orgs/{login}", self.api),
            html: format!("{}/{login}", self.web),
            avatar: self.avatar(login),
        }
    }

    pub(crate) fn team(&self, org: &str, slug: &str) -> TeamUrls {
        TeamUrls {
            api: format!("{}/orgs/{org}/teams/{slug}", self.api),
            html: format!("{}/orgs/{org}/teams/{slug}", self.web),
        }
    }

    pub(crate) fn repo(&self, owner: &str, name: &str) -> RepoUrls {
        RepoUrls {
            api: format!("{}/repos/{owner}/{name}", self.api),
            html: format!("{}/{owner}/{name}", self.web),
            uploads: format!("{}/api/uploads/repos/{owner}/{name}", self.web),
            host: web_host(self.web),
            full_name: format!("{owner}/{name}"),
        }
    }

    pub(crate) fn gist(&self, id: &str) -> GistUrls {
        let api = self.api.trim_end_matches('/');
        let web = self.web.trim_end_matches('/');
        GistUrls {
            api: format!("{api}/gists/{id}"),
            html: format!("{web}/gist/{id}"),
            raw: format!("{web}/raw/{id}"),
            id: id.to_owned(),
        }
    }

    pub(crate) fn installation(&self, id: u64) -> InstallationUrls {
        InstallationUrls {
            access_tokens: format!("{}/app/installations/{id}/access_tokens", self.api),
            html: format!("{}/settings/installations/{id}", self.web),
        }
    }

    pub(crate) fn app_html(&self, slug: &str) -> String {
        format!("{}/apps/{slug}", self.web)
    }

    /// Where the fake serves a workflow job's log.
    pub(crate) fn job_logs(&self, owner: &str, name: &str, id: u64) -> String {
        format!("{}/_ninot/actions/logs/{owner}/{name}/{id}", self.web)
    }

    /// Where the fake serves a workflow run's log archive.
    pub(crate) fn run_log_archive(&self, owner: &str, name: &str, id: u64) -> String {
        format!(
            "{}/_ninot/actions/log-archives/{owner}/{name}/{id}",
            self.web
        )
    }
}

/// The REST API base of a server at `web`, as GitHub Enterprise Server places it.
pub(crate) fn enterprise_api(web: &str) -> String {
    format!("{web}{}", crate::server::ENTERPRISE_PREFIX)
}

/// The GraphQL endpoint of a server at `web`.
pub(crate) fn graphql(web: &str) -> String {
    format!("{web}/api/graphql")
}

/// A runner service endpoint of a server at `web`, under `/_services/`, such as `run/`.
pub(crate) fn service(web: &str, path: &str) -> String {
    format!("{web}/_services/{path}")
}

/// A user's or organization's locations.
pub(crate) struct AccountUrls {
    pub(crate) api: String,
    pub(crate) html: String,
    pub(crate) avatar: String,
}

impl AccountUrls {
    /// A location under the account's API URL, such as `/repos`.
    pub(crate) fn api(&self, rest: &str) -> String {
        format!("{}{rest}", self.api)
    }

    /// An organization member's membership.
    pub(crate) fn membership(&self, login: &str) -> String {
        format!("{}/memberships/{login}", self.api)
    }

    pub(crate) fn public_member(&self, login: &str) -> String {
        format!("{}/public_members/{login}", self.api)
    }

    pub(crate) fn runner_group_runners(&self, id: u64) -> String {
        format!("{}/actions/runner-groups/{id}/runners", self.api)
    }

    pub(crate) fn runner_group_repositories(&self, id: u64) -> String {
        format!("{}/actions/runner-groups/{id}/repositories", self.api)
    }
}

pub(crate) struct TeamUrls {
    pub(crate) api: String,
    pub(crate) html: String,
}

impl TeamUrls {
    pub(crate) fn members(&self) -> String {
        format!("{}/members{{/member}}", self.api)
    }

    pub(crate) fn repositories(&self) -> String {
        format!("{}/repos", self.api)
    }

    pub(crate) fn membership(&self, login: &str) -> String {
        format!("{}/memberships/{login}", self.api)
    }
}

pub(crate) struct GistUrls {
    pub(crate) api: String,
    pub(crate) html: String,
    raw: String,
    id: String,
}

impl GistUrls {
    pub(crate) fn comments(&self) -> String {
        format!("{}/comments", self.api)
    }

    pub(crate) fn commits(&self) -> String {
        format!("{}/commits", self.api)
    }

    pub(crate) fn forks(&self) -> String {
        format!("{}/forks", self.api)
    }

    pub(crate) fn raw(&self, filename: &str) -> String {
        format!("{}/{filename}", self.raw)
    }

    /// The gist's git remote, on a fixed host the fake does not serve.
    pub(crate) fn git(&self) -> String {
        format!("https://gist.github.test/{}.git", self.id)
    }
}

pub(crate) struct InstallationUrls {
    pub(crate) access_tokens: String,
    pub(crate) html: String,
}

/// A repository's locations, and those of the resources it holds.
pub(crate) struct RepoUrls {
    /// Such as `http://host/api/v3/repos/o/r`.
    pub(crate) api: String,
    /// Such as `http://host/o/r`.
    pub(crate) html: String,
    uploads: String,
    host: String,
    full_name: String,
}

impl RepoUrls {
    /// A location under the repository's API URL, such as `/issues{/number}`.
    pub(crate) fn api(&self, rest: &str) -> String {
        format!("{}{rest}", self.api)
    }

    pub(crate) fn clone_url(&self) -> String {
        format!("{}.git", self.html)
    }

    pub(crate) fn git_url(&self) -> String {
        format!("git://{}/{}.git", self.host, self.full_name)
    }

    pub(crate) fn ssh_url(&self) -> String {
        format!("git@{}:{}.git", self.host, self.full_name)
    }

    pub(crate) fn pull(&self, number: u64) -> PullUrls {
        PullUrls {
            api: format!("{}/pulls/{number}", self.api),
            html: format!("{}/pull/{number}", self.html),
            issue: self.issue(number),
        }
    }

    pub(crate) fn issue(&self, number: u64) -> IssueUrls {
        IssueUrls {
            api: format!("{}/issues/{number}", self.api),
            html: format!("{}/issues/{number}", self.html),
        }
    }

    pub(crate) fn issue_comment(&self, id: impl std::fmt::Display) -> String {
        format!("{}/issues/comments/{id}", self.api)
    }

    pub(crate) fn issue_event(&self, id: u64) -> String {
        format!("{}/issues/events/{id}", self.api)
    }

    pub(crate) fn review_comment(&self, id: impl std::fmt::Display) -> String {
        format!("{}/pulls/comments/{id}", self.api)
    }

    /// The `review_comment_url` template of a pull request.
    pub(crate) fn review_comment_template(&self) -> String {
        format!("{}/pulls/comments{{/number}}", self.api)
    }

    pub(crate) fn commit(&self, sha: &str) -> CommitUrls {
        CommitUrls {
            api: format!("{}/commits/{sha}", self.api),
            html: format!("{}/commit/{sha}", self.html),
            git: format!("{}/git/commits/{sha}", self.api),
        }
    }

    /// The combined status of a ref.
    pub(crate) fn statuses(&self, sha: &str) -> String {
        format!("{}/statuses/{sha}", self.api)
    }

    pub(crate) fn status(&self, sha: &str, id: impl std::fmt::Display) -> String {
        format!("{}/statuses/{sha}/{id}", self.api)
    }

    pub(crate) fn git_tree(&self, sha: &str) -> String {
        format!("{}/git/trees/{sha}", self.api)
    }

    /// A git object in its collection, `commits`, `trees`, `blobs` or `tags`.
    pub(crate) fn git_object(&self, collection: &str, sha: &str) -> String {
        format!("{}/git/{collection}/{sha}", self.api)
    }

    /// A git ref by its full name, such as `refs/heads/main`.
    pub(crate) fn git_ref(&self, name: &str) -> String {
        format!("{}/git/{name}", self.api)
    }

    pub(crate) fn tree_html(&self, sha: &str) -> String {
        format!("{}/tree/{sha}", self.html)
    }

    pub(crate) fn blob_html(&self, reference: &str, path: &str) -> String {
        self.entry_html("blob", reference, path)
    }

    /// A path at a ref, shown as a `tree` or a `blob`.
    pub(crate) fn entry_html(&self, kind: &str, reference: &str, path: &str) -> String {
        format!("{}/{kind}/{reference}/{path}", self.html)
    }

    pub(crate) fn raw(&self, reference: &str, path: &str) -> String {
        format!("{}/raw/{reference}/{path}", self.html)
    }

    pub(crate) fn contents(&self, path: &str, reference: &str) -> String {
        format!("{}/contents/{path}?ref={reference}", self.api)
    }

    pub(crate) fn branch(&self, name: &str) -> String {
        format!("{}/branches/{name}", self.api)
    }

    pub(crate) fn branch_protection(&self, name: &str) -> String {
        format!("{}/branches/{name}/protection", self.api)
    }

    pub(crate) fn compare(&self, base: &str, head: &str) -> CompareUrls {
        CompareUrls {
            api: format!("{}/compare/{base}...{head}", self.api),
            html: format!("{}/compare/{base}...{head}", self.html),
        }
    }

    pub(crate) fn tarball(&self, reference: &str) -> String {
        format!("{}/tarball/{reference}", self.api)
    }

    pub(crate) fn zipball(&self, reference: &str) -> String {
        format!("{}/zipball/{reference}", self.api)
    }

    /// A label, its name percent-encoded as GitHub encodes it: `bug :bug:` becomes
    /// `bug%20:bug:`.
    pub(crate) fn label(&self, name: &str) -> String {
        let encoded: String = url::form_urlencoded::byte_serialize(name.as_bytes()).collect();
        format!(
            "{}/labels/{}",
            self.api,
            encoded.replace('+', "%20").replace("%3A", ":")
        )
    }

    pub(crate) fn milestone(&self, number: u64) -> MilestoneUrls {
        MilestoneUrls {
            api: format!("{}/milestones/{number}", self.api),
            html: format!("{}/milestone/{number}", self.html),
        }
    }

    pub(crate) fn check_run(&self, id: impl std::fmt::Display) -> CheckRunUrls {
        CheckRunUrls {
            api: format!("{}/check-runs/{id}", self.api),
            html: format!("{}/runs/{id}", self.html),
        }
    }

    pub(crate) fn check_suite(&self, id: impl std::fmt::Display) -> CheckSuiteUrls {
        CheckSuiteUrls {
            api: format!("{}/check-suites/{id}", self.api),
        }
    }

    /// A workflow by its numeric id or file name.
    pub(crate) fn workflow(&self, id: impl std::fmt::Display) -> String {
        format!("{}/actions/workflows/{id}", self.api)
    }

    pub(crate) fn workflow_badge(&self, id: impl std::fmt::Display) -> String {
        format!("{}/actions/workflows/{id}/badge.svg", self.html)
    }

    pub(crate) fn workflow_run(&self, id: impl std::fmt::Display) -> WorkflowRunUrls {
        WorkflowRunUrls {
            api: format!("{}/actions/runs/{id}", self.api),
            html: format!("{}/actions/runs/{id}", self.html),
        }
    }

    pub(crate) fn workflow_job(
        &self,
        run_id: impl std::fmt::Display,
        id: impl std::fmt::Display,
    ) -> WorkflowJobUrls {
        WorkflowJobUrls {
            api: format!("{}/actions/jobs/{id}", self.api),
            html: format!("{}/actions/runs/{run_id}/job/{id}", self.html),
        }
    }

    pub(crate) fn release(&self, id: u64, tag: &str) -> ReleaseUrls {
        ReleaseUrls {
            api: format!("{}/releases/{id}", self.api),
            html: format!("{}/releases/tag/{tag}", self.html),
            upload: format!("{}/releases/{id}/assets{{?name,label}}", self.uploads),
        }
    }

    pub(crate) fn release_asset(&self, id: u64) -> String {
        format!("{}/releases/assets/{id}", self.api)
    }

    pub(crate) fn release_download(&self, tag: &str, name: &str) -> String {
        format!("{}/releases/download/{tag}/{name}", self.html)
    }

    pub(crate) fn hook(&self, id: u64) -> HookUrls {
        HookUrls {
            api: format!("{}/hooks/{id}", self.api),
        }
    }
}

pub(crate) struct PullUrls {
    pub(crate) api: String,
    pub(crate) html: String,
    /// The pull request's issue.
    pub(crate) issue: IssueUrls,
}

impl PullUrls {
    pub(crate) fn commits(&self) -> String {
        format!("{}/commits", self.api)
    }

    pub(crate) fn review_comments(&self) -> String {
        format!("{}/comments", self.api)
    }

    pub(crate) fn diff(&self) -> String {
        format!("{}.diff", self.html)
    }

    pub(crate) fn patch(&self) -> String {
        format!("{}.patch", self.html)
    }

    pub(crate) fn review_html(&self, id: impl std::fmt::Display) -> String {
        format!("{}#pullrequestreview-{id}", self.html)
    }

    pub(crate) fn review_comment_html(&self, id: impl std::fmt::Display) -> String {
        format!("{}#discussion_r{id}", self.html)
    }

    pub(crate) fn comment_html(&self, id: impl std::fmt::Display) -> String {
        format!("{}#issuecomment-{id}", self.html)
    }
}

pub(crate) struct IssueUrls {
    pub(crate) api: String,
    pub(crate) html: String,
}

impl IssueUrls {
    pub(crate) fn comments(&self) -> String {
        format!("{}/comments", self.api)
    }

    pub(crate) fn events(&self) -> String {
        format!("{}/events", self.api)
    }

    /// The `labels_url` template.
    pub(crate) fn labels(&self) -> String {
        format!("{}/labels{{/name}}", self.api)
    }

    pub(crate) fn comment_html(&self, id: impl std::fmt::Display) -> String {
        format!("{}#issuecomment-{id}", self.html)
    }
}

pub(crate) struct CommitUrls {
    /// Such as `.../repos/o/r/commits/{sha}`.
    pub(crate) api: String,
    pub(crate) html: String,
    /// The git database commit, `.../repos/o/r/git/commits/{sha}`.
    pub(crate) git: String,
}

impl CommitUrls {
    pub(crate) fn comments(&self) -> String {
        format!("{}/comments", self.api)
    }

    pub(crate) fn status(&self) -> String {
        format!("{}/status", self.api)
    }
}

pub(crate) struct CompareUrls {
    pub(crate) api: String,
    pub(crate) html: String,
}

impl CompareUrls {
    pub(crate) fn diff(&self) -> String {
        format!("{}.diff", self.html)
    }

    pub(crate) fn patch(&self) -> String {
        format!("{}.patch", self.html)
    }
}

pub(crate) struct MilestoneUrls {
    pub(crate) api: String,
    pub(crate) html: String,
}

impl MilestoneUrls {
    pub(crate) fn labels(&self) -> String {
        format!("{}/labels", self.api)
    }
}

pub(crate) struct CheckRunUrls {
    pub(crate) api: String,
    pub(crate) html: String,
}

impl CheckRunUrls {
    pub(crate) fn annotations(&self) -> String {
        format!("{}/annotations", self.api)
    }
}

pub(crate) struct CheckSuiteUrls {
    pub(crate) api: String,
}

impl CheckSuiteUrls {
    pub(crate) fn check_runs(&self) -> String {
        format!("{}/check-runs", self.api)
    }
}

pub(crate) struct WorkflowRunUrls {
    pub(crate) api: String,
    pub(crate) html: String,
}

impl WorkflowRunUrls {
    /// A location under the run, such as `/jobs`.
    pub(crate) fn api(&self, rest: &str) -> String {
        format!("{}{rest}", self.api)
    }
}

pub(crate) struct WorkflowJobUrls {
    pub(crate) api: String,
    pub(crate) html: String,
}

pub(crate) struct ReleaseUrls {
    pub(crate) api: String,
    pub(crate) html: String,
    pub(crate) upload: String,
}

impl ReleaseUrls {
    pub(crate) fn assets(&self) -> String {
        format!("{}/assets", self.api)
    }
}

pub(crate) struct HookUrls {
    pub(crate) api: String,
}

impl HookUrls {
    pub(crate) fn deliveries(&self) -> String {
        format!("{}/deliveries", self.api)
    }

    pub(crate) fn pings(&self) -> String {
        format!("{}/pings", self.api)
    }

    pub(crate) fn test(&self) -> String {
        format!("{}/test", self.api)
    }
}

#[cfg(test)]
mod tests {
    use super::Urls;

    const API: &str = "http://h:1/api/v3";
    const WEB: &str = "http://h:1";

    fn urls() -> Urls<'static> {
        Urls::new(API, WEB)
    }

    #[test]
    fn accounts() {
        let user = urls().user("alice");
        assert_eq!(user.api, "http://h:1/api/v3/users/alice");
        assert_eq!(user.html, "http://h:1/alice");
        assert_eq!(user.avatar, "http://h:1/avatars/alice");
        assert_eq!(urls().org("acme").api, "http://h:1/api/v3/orgs/acme");
        let team = urls().team("acme", "core");
        assert_eq!(team.api, "http://h:1/api/v3/orgs/acme/teams/core");
        assert_eq!(team.html, "http://h:1/orgs/acme/teams/core");
        assert_eq!(
            team.members(),
            "http://h:1/api/v3/orgs/acme/teams/core/members{/member}"
        );
    }

    #[test]
    fn repository() {
        let repo = urls().repo("o", "r");
        assert_eq!(repo.api, "http://h:1/api/v3/repos/o/r");
        assert_eq!(repo.html, "http://h:1/o/r");
        assert_eq!(repo.clone_url(), "http://h:1/o/r.git");
        assert_eq!(repo.git_url(), "git://h:1/o/r.git");
        assert_eq!(repo.ssh_url(), "git@h:1:o/r.git");
        assert_eq!(
            repo.branch("a/b"),
            "http://h:1/api/v3/repos/o/r/branches/a/b"
        );
        assert_eq!(
            repo.contents("dir/f.txt", "main"),
            "http://h:1/api/v3/repos/o/r/contents/dir/f.txt?ref=main"
        );
        assert_eq!(repo.blob_html("abc", "f"), "http://h:1/o/r/blob/abc/f");
        assert_eq!(repo.raw("abc", "f"), "http://h:1/o/r/raw/abc/f");
        let compare = repo.compare("a", "b");
        assert_eq!(compare.api, "http://h:1/api/v3/repos/o/r/compare/a...b");
        assert_eq!(compare.patch(), "http://h:1/o/r/compare/a...b.patch");
    }

    #[test]
    fn pull_and_issue() {
        let pull = urls().repo("o", "r").pull(7);
        assert_eq!(pull.api, "http://h:1/api/v3/repos/o/r/pulls/7");
        assert_eq!(pull.html, "http://h:1/o/r/pull/7");
        assert_eq!(pull.diff(), "http://h:1/o/r/pull/7.diff");
        assert_eq!(
            pull.commits(),
            "http://h:1/api/v3/repos/o/r/pulls/7/commits"
        );
        assert_eq!(
            pull.review_comments(),
            "http://h:1/api/v3/repos/o/r/pulls/7/comments"
        );
        assert_eq!(
            pull.review_html(3),
            "http://h:1/o/r/pull/7#pullrequestreview-3"
        );
        assert_eq!(pull.issue.api, "http://h:1/api/v3/repos/o/r/issues/7");
        assert_eq!(
            pull.issue.labels(),
            "http://h:1/api/v3/repos/o/r/issues/7/labels{/name}"
        );
        assert_eq!(
            pull.issue.comment_html(9),
            "http://h:1/o/r/issues/7#issuecomment-9"
        );
    }

    #[test]
    fn commit_and_git() {
        let repo = urls().repo("o", "r");
        let commit = repo.commit("abc");
        assert_eq!(commit.api, "http://h:1/api/v3/repos/o/r/commits/abc");
        assert_eq!(commit.html, "http://h:1/o/r/commit/abc");
        assert_eq!(commit.git, "http://h:1/api/v3/repos/o/r/git/commits/abc");
        assert_eq!(
            commit.status(),
            "http://h:1/api/v3/repos/o/r/commits/abc/status"
        );
        assert_eq!(
            repo.git_tree("t"),
            "http://h:1/api/v3/repos/o/r/git/trees/t"
        );
        assert_eq!(
            repo.git_ref("refs/heads/main"),
            "http://h:1/api/v3/repos/o/r/git/refs/heads/main"
        );
    }

    #[test]
    fn labels_are_encoded_as_github_encodes_them() {
        let repo = urls().repo("o", "r");
        assert_eq!(
            repo.label("bug :bug:"),
            "http://h:1/api/v3/repos/o/r/labels/bug%20:bug:"
        );
        assert_eq!(
            repo.label("\u{1f692} wontfix"),
            "http://h:1/api/v3/repos/o/r/labels/%F0%9F%9A%92%20wontfix"
        );
        assert_eq!(
            repo.label("a/b+c"),
            "http://h:1/api/v3/repos/o/r/labels/a%2Fb%2Bc"
        );
    }

    #[test]
    fn actions_and_checks() {
        let repo = urls().repo("o", "r");
        assert_eq!(
            repo.check_run(5).annotations(),
            "http://h:1/api/v3/repos/o/r/check-runs/5/annotations"
        );
        assert_eq!(repo.check_run(5).html, "http://h:1/o/r/runs/5");
        assert_eq!(
            repo.check_suite(6).check_runs(),
            "http://h:1/api/v3/repos/o/r/check-suites/6/check-runs"
        );
        let job = repo.workflow_job(1, 2);
        assert_eq!(job.api, "http://h:1/api/v3/repos/o/r/actions/jobs/2");
        assert_eq!(job.html, "http://h:1/o/r/actions/runs/1/job/2");
        let release = repo.release(4, "v1");
        assert_eq!(
            release.upload,
            "http://h:1/api/uploads/repos/o/r/releases/4/assets{?name,label}"
        );
        assert_eq!(release.html, "http://h:1/o/r/releases/tag/v1");
    }
}
