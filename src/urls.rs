//! HTTP locations of GitHub resources, shaped as GitHub shapes them.
//!
//! [`Urls`] holds the API and web base URLs a client addressed; its methods return one
//! resource's locations. `api` URLs sit under the API base, `html` URLs under the web base.
//!
//! API URLs fill the path template of an operation in the pinned OpenAPI description, so no
//! API path is typed here. URI-template fields, such as `issues_url`, append the expression
//! GitHub's schema example shows, such as `{/number}`, to an operation's path. Web URLs have
//! no machine-readable source; each cites the schema example or page it copies.

use std::fmt::Display;

use crate::operation::{Operation, ops};
use crate::render::web_host;
use crate::server::UPLOADS_PREFIX;

include!(concat!(env!("OUT_DIR"), "/multi_segment.rs"));

/// A path parameter's name and value.
type Param<'p> = (&'p str, &'p dyn Display);

/// `O`'s path template under `base`, each `{name}` replaced by its parameter.
///
/// # Panics
///
/// Panics when a parameter is missing from `params` or `params` names one the path lacks.
fn api_url<O: Operation>(base: &str, params: &[Param]) -> String {
    let template = O::INFO.path;
    let mut url = String::from(base);
    let mut used = 0;
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let end = start + rest[start..].find('}').expect("unclosed path parameter");
        let name = &rest[start + 1..end];
        let (_, value) = params
            .iter()
            .find(|(key, _)| *key == name)
            .unwrap_or_else(|| panic!("no value for {{{name}}} in {template}"));
        used += 1;
        url.push_str(&rest[..start]);
        let value = value.to_string();
        if MULTI_SEGMENT.contains(&(template, name)) {
            url.push_str(&value);
        } else {
            url.push_str(&encode_segment(&value));
        }
        rest = &rest[end + 1..];
    }
    url.push_str(rest);
    assert_eq!(used, params.len(), "unused parameters for {template}");
    url
}

/// A single path segment, percent-encoded as GitHub encodes label names: `bug :bug:` becomes
/// `bug%20:bug:`.
fn encode_segment(value: &str) -> String {
    let encoded: String = url::form_urlencoded::byte_serialize(value.as_bytes()).collect();
    encoded.replace('+', "%20").replace("%3A", ":")
}

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

    /// `O`'s path under the API base, filled with `params`, such as
    /// `api_for::<ops::UsersGetByUsername>(&[("username", &"alice")])`.
    pub(crate) fn api_for<O: Operation>(&self, params: &[Param]) -> String {
        api_url::<O>(self.api, params)
    }

    /// A page under the web base, such as `/settings`.
    pub(crate) fn web(&self, path: &str) -> String {
        format!("{}{path}", self.web)
    }

    pub(crate) fn user(&self, login: &str) -> AccountUrls {
        AccountUrls {
            api: self.api_for::<ops::UsersGetByUsername>(&[("username", &login)]),
            html: self.account_html(login),
            avatar: self.avatar(login),
            base: self.api.to_owned(),
            login: login.to_owned(),
        }
    }

    pub(crate) fn org(&self, login: &str) -> AccountUrls {
        AccountUrls {
            api: self.api_for::<ops::OrgsGet>(&[("org", &login)]),
            html: self.account_html(login),
            avatar: self.avatar(login),
            base: self.api.to_owned(),
            login: login.to_owned(),
        }
    }

    pub(crate) fn team(&self, org: &str, slug: &str) -> TeamUrls {
        TeamUrls {
            api: self.api_for::<ops::TeamsGetByName>(&[("org", &org), ("team_slug", &slug)]),
            html: self.team_html(org, slug),
            base: self.api.to_owned(),
            org: org.to_owned(),
            slug: slug.to_owned(),
        }
    }

    pub(crate) fn repo(&self, owner: &str, name: &str) -> RepoUrls {
        let repo = Repo {
            base: self.api.to_owned(),
            uploads: format!("{}{UPLOADS_PREFIX}", self.web),
            owner: owner.to_owned(),
            name: name.to_owned(),
        };
        RepoUrls {
            api: repo.api_for::<ops::ReposGet>(&[]),
            html: self.repo_html(owner, name),
            host: web_host(self.web),
            full_name: format!("{owner}/{name}"),
            repo,
        }
    }

    pub(crate) fn gist(&self, id: &str) -> GistUrls {
        let base = self.api.trim_end_matches('/');
        GistUrls {
            api: api_url::<ops::GistsGet>(base, &[("gist_id", &id)]),
            html: self.gist_html(id),
            raw: self.gist_raw(id),
            base: base.to_owned(),
            id: id.to_owned(),
        }
    }

    pub(crate) fn installation(&self, id: u64) -> InstallationUrls {
        InstallationUrls {
            // Source: OpenAPI schema installation, access_tokens_url example.
            access_tokens: self
                .api_for::<ops::AppsCreateInstallationAccessToken>(&[("installation_id", &id)]),
            html: self.installation_html(id),
        }
    }
}

/// Web URLs of [`Urls`]. GitHub publishes no list of them; each cites the shape it copies.
impl Urls<'_> {
    /// Where the fake serves an account's avatar.
    pub(crate) fn avatar(&self, login: &str) -> String {
        format!("{}/avatars/{login}", self.web)
    }

    /// Source: OpenAPI schema simple-user, html_url example `https://github.com/octocat`.
    fn account_html(&self, login: &str) -> String {
        format!("{}/{login}", self.web)
    }

    /// Source: OpenAPI schema team, html_url example `https://github.com/orgs/rails/teams/core`.
    fn team_html(&self, org: &str, slug: &str) -> String {
        format!("{}/orgs/{org}/teams/{slug}", self.web)
    }

    /// Source: OpenAPI schema repository, html_url example
    /// `https://github.com/octocat/Hello-World`.
    fn repo_html(&self, owner: &str, name: &str) -> String {
        format!("{}/{owner}/{name}", self.web)
    }

    /// Source: OpenAPI schema installation, html_url example
    /// `https://github.com/organizations/github/settings/installations/1`, without the
    /// account prefix.
    fn installation_html(&self, id: u64) -> String {
        format!("{}/settings/installations/{id}", self.web)
    }

    /// Where the fake places a gist's page. GitHub serves gists on their own host: OpenAPI
    /// example base-gist, html_url `https://gist.github.com/aa5a315d61ae9438b18d`.
    fn gist_html(&self, id: &str) -> String {
        format!("{}/gist/{id}", self.web.trim_end_matches('/'))
    }

    /// Where the fake serves a gist's files, under [`GistUrls::raw`].
    fn gist_raw(&self, id: &str) -> String {
        format!("{}/raw/{id}", self.web.trim_end_matches('/'))
    }

    /// Source: OpenAPI schema integration, html_url example `https://github.com/apps/super-ci`.
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
    base: String,
    login: String,
}

impl AccountUrls {
    /// A location under the account's API URL, such as `/repos`.
    pub(crate) fn api(&self, rest: &str) -> String {
        format!("{}{rest}", self.api)
    }

    /// An organization member's membership.
    pub(crate) fn membership(&self, login: &str) -> String {
        api_url::<ops::OrgsGetMembershipForUser>(
            &self.base,
            &[("org", &self.login), ("username", &login)],
        )
    }

    pub(crate) fn public_member(&self, login: &str) -> String {
        api_url::<ops::OrgsCheckPublicMembershipForUser>(
            &self.base,
            &[("org", &self.login), ("username", &login)],
        )
    }

    pub(crate) fn runner_group_runners(&self, id: u64) -> String {
        api_url::<ops::ActionsListSelfHostedRunnersInGroupForOrg>(
            &self.base,
            &[("org", &self.login), ("runner_group_id", &id)],
        )
    }

    pub(crate) fn runner_group_repositories(&self, id: u64) -> String {
        api_url::<ops::ActionsListRepoAccessToSelfHostedRunnerGroupInOrg>(
            &self.base,
            &[("org", &self.login), ("runner_group_id", &id)],
        )
    }
}

pub(crate) struct TeamUrls {
    pub(crate) api: String,
    pub(crate) html: String,
    base: String,
    org: String,
    slug: String,
}

impl TeamUrls {
    fn api_for<O: Operation>(&self, params: &[Param]) -> String {
        let mut all: Vec<Param> = vec![("org", &self.org), ("team_slug", &self.slug)];
        all.extend_from_slice(params);
        api_url::<O>(&self.base, &all)
    }

    /// Source: OpenAPI schema team-full, members_url example
    /// `https://api.github.com/organizations/1/team/1/members{/member}`.
    pub(crate) fn members(&self) -> String {
        format!(
            "{}{{/member}}",
            self.api_for::<ops::TeamsListMembersInOrg>(&[])
        )
    }

    pub(crate) fn repositories(&self) -> String {
        self.api_for::<ops::TeamsListReposInOrg>(&[])
    }

    pub(crate) fn membership(&self, login: &str) -> String {
        self.api_for::<ops::TeamsGetMembershipForUserInOrg>(&[("username", &login)])
    }
}

pub(crate) struct GistUrls {
    pub(crate) api: String,
    pub(crate) html: String,
    raw: String,
    base: String,
    id: String,
}

impl GistUrls {
    pub(crate) fn comments(&self) -> String {
        api_url::<ops::GistsListComments>(&self.base, &[("gist_id", &self.id)])
    }

    pub(crate) fn commits(&self) -> String {
        api_url::<ops::GistsListCommits>(&self.base, &[("gist_id", &self.id)])
    }

    pub(crate) fn forks(&self) -> String {
        api_url::<ops::GistsListForks>(&self.base, &[("gist_id", &self.id)])
    }
}

/// Web URLs of a gist.
impl GistUrls {
    /// Where the fake serves a gist file.
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

/// A repository's API base and coordinates, which its resources' URLs share.
#[derive(Clone)]
struct Repo {
    /// The API base, such as `http://host/api/v3`.
    base: String,
    /// The uploads base, such as `http://host/api/uploads`.
    uploads: String,
    owner: String,
    name: String,
}

impl Repo {
    /// `O`'s path under the API base, with `owner` and `repo` filled in.
    fn api_for<O: Operation>(&self, params: &[Param]) -> String {
        api_url::<O>(&self.base, &self.with_repo(params))
    }

    /// `O`'s path under the uploads base, with `owner` and `repo` filled in.
    fn uploads_for<O: Operation>(&self, params: &[Param]) -> String {
        api_url::<O>(&self.uploads, &self.with_repo(params))
    }

    fn with_repo<'p>(&'p self, params: &[Param<'p>]) -> Vec<Param<'p>> {
        let mut all: Vec<Param> = vec![("owner", &self.owner), ("repo", &self.name)];
        all.extend_from_slice(params);
        all
    }
}

/// A repository's locations, and those of the resources it holds.
pub(crate) struct RepoUrls {
    /// Such as `http://host/api/v3/repos/o/r`.
    pub(crate) api: String,
    /// Such as `http://host/o/r`.
    pub(crate) html: String,
    host: String,
    full_name: String,
    repo: Repo,
}

impl RepoUrls {
    /// A location under the repository's API URL, such as `/issues{/number}`.
    pub(crate) fn api(&self, rest: &str) -> String {
        format!("{}{rest}", self.api)
    }

    pub(crate) fn pull(&self, number: u64) -> PullUrls {
        PullUrls {
            api: self
                .repo
                .api_for::<ops::PullsGet>(&[("pull_number", &number)]),
            html: self.pull_html(number),
            issue: self.issue(number),
            repo: self.repo.clone(),
            number,
        }
    }

    pub(crate) fn issue(&self, number: u64) -> IssueUrls {
        IssueUrls {
            api: self
                .repo
                .api_for::<ops::IssuesGet>(&[("issue_number", &number)]),
            html: self.issue_html(number),
            repo: self.repo.clone(),
            number,
        }
    }

    pub(crate) fn issue_comment(&self, id: impl Display) -> String {
        self.repo
            .api_for::<ops::IssuesGetComment>(&[("comment_id", &id)])
    }

    pub(crate) fn issue_event(&self, id: u64) -> String {
        self.repo
            .api_for::<ops::IssuesGetEvent>(&[("event_id", &id)])
    }

    pub(crate) fn review_comment(&self, id: impl Display) -> String {
        self.repo
            .api_for::<ops::PullsGetReviewComment>(&[("comment_id", &id)])
    }

    /// The `review_comment_url` template of a pull request.
    ///
    /// Source: OpenAPI schema pull-request, review_comment_url example
    /// `https://api.github.com/repos/octocat/Hello-World/pulls/comments{/number}`.
    pub(crate) fn review_comment_template(&self) -> String {
        format!(
            "{}{{/number}}",
            self.repo
                .api_for::<ops::PullsListReviewCommentsForRepo>(&[])
        )
    }

    pub(crate) fn commit(&self, sha: &str) -> CommitUrls {
        CommitUrls {
            api: self.repo.api_for::<ops::ReposGetCommit>(&[("ref", &sha)]),
            html: self.commit_html(sha),
            git: self
                .repo
                .api_for::<ops::GitGetCommit>(&[("commit_sha", &sha)]),
            repo: self.repo.clone(),
            sha: sha.to_owned(),
        }
    }

    /// The statuses of a ref. GitHub links the `POST` operation's path: no `GET` has it.
    ///
    /// Source: OpenAPI schema pull-request, statuses_url example
    /// `https://api.github.com/repos/octocat/Hello-World/statuses/6dcb09b5b57875f334f61aebed695e2e4193db5e`.
    pub(crate) fn statuses(&self, sha: &str) -> String {
        self.repo
            .api_for::<ops::ReposCreateCommitStatus>(&[("sha", &sha)])
    }

    /// A commit status's `url`, which names its commit and not the status: the spec has no
    /// path for one status.
    ///
    /// Source: OpenAPI example status, url
    /// `https://api.github.com/repos/octocat/Hello-World/statuses/6dcb09b5b57875f334f61aebed695e2e4193db5e`.
    pub(crate) fn status(&self, sha: &str, _id: impl Display) -> String {
        self.statuses(sha)
    }

    pub(crate) fn git_tree(&self, sha: &str) -> String {
        self.repo.api_for::<ops::GitGetTree>(&[("tree_sha", &sha)])
    }

    /// A git object in its collection, `commits`, `trees`, `blobs` or `tags`.
    pub(crate) fn git_object(&self, collection: &str, sha: &str) -> String {
        match collection {
            "commits" => self
                .repo
                .api_for::<ops::GitGetCommit>(&[("commit_sha", &sha)]),
            "trees" => self.git_tree(sha),
            "blobs" => self.repo.api_for::<ops::GitGetBlob>(&[("file_sha", &sha)]),
            "tags" => self.repo.api_for::<ops::GitGetTag>(&[("tag_sha", &sha)]),
            other => panic!("no git object collection {other}"),
        }
    }

    /// A git ref by its full name, such as `refs/heads/main`. GitHub links the path of
    /// `git/update-ref`: `git/get-ref` reads `/git/ref/{ref}`, singular.
    ///
    /// Source: OpenAPI example git-ref-items, url
    /// `https://api.github.com/repos/octocat/Hello-World/git/refs/heads/feature-a`.
    pub(crate) fn git_ref(&self, name: &str) -> String {
        let name = name
            .strip_prefix("refs/")
            .unwrap_or_else(|| panic!("{name} is not a full ref name"));
        self.repo.api_for::<ops::GitUpdateRef>(&[("ref", &name)])
    }

    pub(crate) fn contents(&self, path: &str, reference: &str) -> String {
        format!(
            "{}?ref={reference}",
            self.repo
                .api_for::<ops::ReposGetContent>(&[("path", &path)])
        )
    }

    pub(crate) fn branch(&self, name: &str) -> String {
        self.repo
            .api_for::<ops::ReposGetBranch>(&[("branch", &name)])
    }

    pub(crate) fn branch_protection(&self, name: &str) -> String {
        self.repo
            .api_for::<ops::ReposGetBranchProtection>(&[("branch", &name)])
    }

    pub(crate) fn compare(&self, base: &str, head: &str) -> CompareUrls {
        let basehead = format!("{base}...{head}");
        CompareUrls {
            api: self
                .repo
                .api_for::<ops::ReposCompareCommits>(&[("basehead", &basehead)]),
            html: self.compare_html(&basehead),
        }
    }

    pub(crate) fn tarball(&self, reference: &str) -> String {
        self.repo
            .api_for::<ops::ReposDownloadTarballArchive>(&[("ref", &reference)])
    }

    pub(crate) fn zipball(&self, reference: &str) -> String {
        self.repo
            .api_for::<ops::ReposDownloadZipballArchive>(&[("ref", &reference)])
    }

    /// A label, its name percent-encoded as GitHub encodes it: `bug :bug:` becomes
    /// `bug%20:bug:`.
    pub(crate) fn label(&self, name: &str) -> String {
        self.repo.api_for::<ops::IssuesGetLabel>(&[("name", &name)])
    }

    pub(crate) fn milestone(&self, number: u64) -> MilestoneUrls {
        MilestoneUrls {
            api: self
                .repo
                .api_for::<ops::IssuesGetMilestone>(&[("milestone_number", &number)]),
            html: self.milestone_html(number),
            repo: self.repo.clone(),
            number,
        }
    }

    pub(crate) fn check_run(&self, id: impl Display) -> CheckRunUrls {
        CheckRunUrls {
            api: self
                .repo
                .api_for::<ops::ChecksGet>(&[("check_run_id", &id)]),
            html: self.check_run_html(&id),
            repo: self.repo.clone(),
            id: id.to_string(),
        }
    }

    pub(crate) fn check_suite(&self, id: impl Display) -> CheckSuiteUrls {
        CheckSuiteUrls {
            api: self
                .repo
                .api_for::<ops::ChecksGetSuite>(&[("check_suite_id", &id)]),
            repo: self.repo.clone(),
            id: id.to_string(),
        }
    }

    /// A workflow by its numeric id or file name.
    pub(crate) fn workflow(&self, id: impl Display) -> String {
        self.repo
            .api_for::<ops::ActionsGetWorkflow>(&[("workflow_id", &id)])
    }

    pub(crate) fn workflow_run(&self, id: impl Display) -> WorkflowRunUrls {
        WorkflowRunUrls {
            api: self
                .repo
                .api_for::<ops::ActionsGetWorkflowRun>(&[("run_id", &id)]),
            html: self.workflow_run_html(&id),
        }
    }

    pub(crate) fn workflow_job(&self, run_id: impl Display, id: impl Display) -> WorkflowJobUrls {
        WorkflowJobUrls {
            api: self
                .repo
                .api_for::<ops::ActionsGetJobForWorkflowRun>(&[("job_id", &id)]),
            html: self.workflow_job_html(&run_id, &id),
        }
    }

    pub(crate) fn release(&self, id: u64, tag: &str) -> ReleaseUrls {
        ReleaseUrls {
            api: self
                .repo
                .api_for::<ops::ReposGetRelease>(&[("release_id", &id)]),
            html: self.release_html(tag),
            // Source: OpenAPI example release, upload_url
            // `https://uploads.github.com/repos/octocat/Hello-World/releases/1/assets{?name,label}`.
            upload: format!(
                "{}{{?name,label}}",
                self.repo
                    .uploads_for::<ops::ReposUploadReleaseAsset>(&[("release_id", &id)])
            ),
            repo: self.repo.clone(),
            id,
        }
    }

    pub(crate) fn release_asset(&self, id: u64) -> String {
        self.repo
            .api_for::<ops::ReposGetReleaseAsset>(&[("asset_id", &id)])
    }

    pub(crate) fn hook(&self, id: u64) -> HookUrls {
        HookUrls {
            api: self
                .repo
                .api_for::<ops::ReposGetWebhook>(&[("hook_id", &id)]),
            repo: self.repo.clone(),
            id,
        }
    }
}

/// Web and git URLs of a repository. GitHub publishes no list of them; each cites the shape
/// it copies.
impl RepoUrls {
    /// Source: OpenAPI schema repository, clone_url example
    /// `https://github.com/octocat/Hello-World.git`.
    pub(crate) fn clone_url(&self) -> String {
        format!("{}.git", self.html)
    }

    /// Source: OpenAPI schema repository, git_url example `git:github.com/octocat/Hello-World.git`,
    /// with the `git://` scheme GitHub serves.
    pub(crate) fn git_url(&self) -> String {
        format!("git://{}/{}.git", self.host, self.full_name)
    }

    /// Source: OpenAPI schema repository, ssh_url example `git@github.com:octocat/Hello-World.git`.
    pub(crate) fn ssh_url(&self) -> String {
        format!("git@{}:{}.git", self.host, self.full_name)
    }

    /// Source: OpenAPI schema pull-request, html_url example
    /// `https://github.com/octocat/Hello-World/pull/1347`.
    fn pull_html(&self, number: u64) -> String {
        format!("{}/pull/{number}", self.html)
    }

    /// Source: OpenAPI schema secret-scanning-location-issue-comment, html_url example
    /// `https://github.com/octocat/Hello-World/issues/1#issuecomment-1081119451`.
    fn issue_html(&self, number: u64) -> String {
        format!("{}/issues/{number}", self.html)
    }

    /// Source: OpenAPI schema commit, html_url example
    /// `https://github.com/octocat/Hello-World/commit/6dcb09b5b57875f334f61aebed695e2e4193db5e`.
    fn commit_html(&self, sha: &str) -> String {
        format!("{}/commit/{sha}", self.html)
    }

    /// Source: OpenAPI schema codespace-export-details, html_url example
    /// `https://github.com/octocat/hello-world/tree/:branch`.
    pub(crate) fn tree_html(&self, sha: &str) -> String {
        format!("{}/tree/{sha}", self.html)
    }

    /// Source: OpenAPI schema secret-scanning-location-commit, html_url example
    /// `https://github.com/octocat/Hello-World/blob/af5626b/example/secrets.txt#L1-L1`.
    pub(crate) fn blob_html(&self, reference: &str, path: &str) -> String {
        self.entry_html("blob", reference, path)
    }

    /// A path at a ref, shown as a `tree` or a `blob`.
    pub(crate) fn entry_html(&self, kind: &str, reference: &str, path: &str) -> String {
        format!("{}/{kind}/{reference}/{path}", self.html)
    }

    /// Source: OpenAPI schema diff-entry, raw_url example
    /// `https://github.com/octocat/Hello-World/raw/6dcb09b5b57875f334f61aebed695e2e4193db5e/file1.txt`.
    pub(crate) fn raw(&self, reference: &str, path: &str) -> String {
        format!("{}/raw/{reference}/{path}", self.html)
    }

    /// Source: OpenAPI schema commit-comparison, diff_url example
    /// `https://github.com/octocat/Hello-World/compare/master...topic.diff`, without `.diff`.
    fn compare_html(&self, basehead: &str) -> String {
        format!("{}/compare/{basehead}", self.html)
    }

    /// Where the fake places a milestone's page. OpenAPI example milestone shows
    /// `https://github.com/octocat/Hello-World/milestones/v1.0`.
    fn milestone_html(&self, number: u64) -> String {
        format!("{}/milestone/{number}", self.html)
    }

    /// Source: OpenAPI schema check-run, html_url example
    /// `https://github.com/github/hello-world/runs/4`.
    fn check_run_html(&self, id: &dyn Display) -> String {
        format!("{}/runs/{id}", self.html)
    }

    /// Where the fake places a workflow's status badge. OpenAPI example workflow shows
    /// `https://github.com/octo-org/octo-repo/workflows/CI/badge.svg`.
    pub(crate) fn workflow_badge(&self, id: impl Display) -> String {
        format!("{}/actions/workflows/{id}/badge.svg", self.html)
    }

    /// Source: OpenAPI example workflow-run, html_url
    /// `https://github.com/octo-org/octo-repo/actions/runs/30433642`.
    fn workflow_run_html(&self, id: &dyn Display) -> String {
        format!("{}/actions/runs/{id}", self.html)
    }

    /// Where the fake places a workflow job's page. OpenAPI example job shows
    /// `https://github.com/octo-org/octo-repo/runs/29679449/jobs/399444496`.
    fn workflow_job_html(&self, run_id: &dyn Display, id: &dyn Display) -> String {
        format!("{}/actions/runs/{run_id}/job/{id}", self.html)
    }

    /// Where the fake places a release's page. OpenAPI example release shows
    /// `https://github.com/octocat/Hello-World/releases/v1.0.0`.
    fn release_html(&self, tag: &str) -> String {
        format!("{}/releases/tag/{tag}", self.html)
    }

    /// Source: OpenAPI example runner-application-items, download_url
    /// `https://github.com/actions/runner/releases/download/v2.164.0/actions-runner-osx-x64-2.164.0.tar.gz`.
    pub(crate) fn release_download(&self, tag: &str, name: &str) -> String {
        format!("{}/releases/download/{tag}/{name}", self.html)
    }
}

pub(crate) struct PullUrls {
    pub(crate) api: String,
    pub(crate) html: String,
    /// The pull request's issue.
    pub(crate) issue: IssueUrls,
    repo: Repo,
    number: u64,
}

impl PullUrls {
    pub(crate) fn commits(&self) -> String {
        self.repo
            .api_for::<ops::PullsListCommits>(&[("pull_number", &self.number)])
    }

    pub(crate) fn review_comments(&self) -> String {
        self.repo
            .api_for::<ops::PullsListReviewComments>(&[("pull_number", &self.number)])
    }
}

/// Web URLs of a pull request.
impl PullUrls {
    /// Source: OpenAPI schema pull-request, diff_url example
    /// `https://github.com/octocat/Hello-World/pull/1347.diff`.
    pub(crate) fn diff(&self) -> String {
        format!("{}.diff", self.html)
    }

    /// Source: OpenAPI schema pull-request, patch_url example
    /// `https://github.com/octocat/Hello-World/pull/1347.patch`.
    pub(crate) fn patch(&self) -> String {
        format!("{}.patch", self.html)
    }

    /// Source: OpenAPI schema pull-request-review, html_url example
    /// `https://github.com/octocat/Hello-World/pull/12#pullrequestreview-80`.
    pub(crate) fn review_html(&self, id: impl Display) -> String {
        format!("{}#pullrequestreview-{id}", self.html)
    }

    /// Source: OpenAPI schema secret-scanning-location-pull-request-review-comment, html_url
    /// example `https://github.com/octocat/Hello-World/pull/2846#discussion_r12`.
    pub(crate) fn review_comment_html(&self, id: impl Display) -> String {
        format!("{}#discussion_r{id}", self.html)
    }

    /// Source: OpenAPI schema secret-scanning-location-pull-request-comment, html_url example
    /// `https://github.com/octocat/Hello-World/pull/2846#issuecomment-1081119451`.
    pub(crate) fn comment_html(&self, id: impl Display) -> String {
        format!("{}#issuecomment-{id}", self.html)
    }
}

pub(crate) struct IssueUrls {
    pub(crate) api: String,
    pub(crate) html: String,
    repo: Repo,
    number: u64,
}

impl IssueUrls {
    pub(crate) fn comments(&self) -> String {
        self.repo
            .api_for::<ops::IssuesListComments>(&[("issue_number", &self.number)])
    }

    pub(crate) fn events(&self) -> String {
        self.repo
            .api_for::<ops::IssuesListEvents>(&[("issue_number", &self.number)])
    }

    /// The `labels_url` template.
    ///
    /// Source: OpenAPI example issue-items, labels_url
    /// `https://api.github.com/repos/octocat/Hello-World/issues/1347/labels{/name}`.
    pub(crate) fn labels(&self) -> String {
        format!(
            "{}{{/name}}",
            self.repo
                .api_for::<ops::IssuesListLabelsOnIssue>(&[("issue_number", &self.number)])
        )
    }

    /// Source: OpenAPI schema secret-scanning-location-issue-comment, html_url example
    /// `https://github.com/octocat/Hello-World/issues/1#issuecomment-1081119451`.
    pub(crate) fn comment_html(&self, id: impl Display) -> String {
        format!("{}#issuecomment-{id}", self.html)
    }
}

pub(crate) struct CommitUrls {
    /// Such as `.../repos/o/r/commits/{sha}`.
    pub(crate) api: String,
    pub(crate) html: String,
    /// The git database commit, `.../repos/o/r/git/commits/{sha}`.
    pub(crate) git: String,
    repo: Repo,
    sha: String,
}

impl CommitUrls {
    pub(crate) fn comments(&self) -> String {
        self.repo
            .api_for::<ops::ReposListCommentsForCommit>(&[("commit_sha", &self.sha)])
    }

    pub(crate) fn status(&self) -> String {
        self.repo
            .api_for::<ops::ReposGetCombinedStatusForRef>(&[("ref", &self.sha)])
    }
}

pub(crate) struct CompareUrls {
    pub(crate) api: String,
    pub(crate) html: String,
}

/// Web URLs of a comparison.
impl CompareUrls {
    /// Source: OpenAPI schema commit-comparison, diff_url example
    /// `https://github.com/octocat/Hello-World/compare/master...topic.diff`.
    pub(crate) fn diff(&self) -> String {
        format!("{}.diff", self.html)
    }

    /// Source: OpenAPI schema commit-comparison, patch_url example
    /// `https://github.com/octocat/Hello-World/compare/master...topic.patch`.
    pub(crate) fn patch(&self) -> String {
        format!("{}.patch", self.html)
    }
}

pub(crate) struct MilestoneUrls {
    pub(crate) api: String,
    pub(crate) html: String,
    repo: Repo,
    number: u64,
}

impl MilestoneUrls {
    pub(crate) fn labels(&self) -> String {
        self.repo
            .api_for::<ops::IssuesListLabelsForMilestone>(&[("milestone_number", &self.number)])
    }
}

pub(crate) struct CheckRunUrls {
    pub(crate) api: String,
    pub(crate) html: String,
    repo: Repo,
    id: String,
}

impl CheckRunUrls {
    pub(crate) fn annotations(&self) -> String {
        self.repo
            .api_for::<ops::ChecksListAnnotations>(&[("check_run_id", &self.id)])
    }
}

pub(crate) struct CheckSuiteUrls {
    pub(crate) api: String,
    repo: Repo,
    id: String,
}

impl CheckSuiteUrls {
    pub(crate) fn check_runs(&self) -> String {
        self.repo
            .api_for::<ops::ChecksListForSuite>(&[("check_suite_id", &self.id)])
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
    repo: Repo,
    id: u64,
}

impl ReleaseUrls {
    pub(crate) fn assets(&self) -> String {
        self.repo
            .api_for::<ops::ReposListReleaseAssets>(&[("release_id", &self.id)])
    }
}

pub(crate) struct HookUrls {
    pub(crate) api: String,
    repo: Repo,
    id: u64,
}

impl HookUrls {
    pub(crate) fn deliveries(&self) -> String {
        self.repo
            .api_for::<ops::ReposListWebhookDeliveries>(&[("hook_id", &self.id)])
    }

    pub(crate) fn pings(&self) -> String {
        self.repo
            .api_for::<ops::ReposPingWebhook>(&[("hook_id", &self.id)])
    }

    /// GitHub's `test_url`, which differs from the path of `repos/test-push-webhook`
    /// (`/tests`); the spec has no operation at `/test`.
    ///
    /// Source: OpenAPI schema hook, test_url example
    /// `https://api.github.com/repos/octocat/Hello-World/hooks/1/test`.
    pub(crate) fn test(&self) -> String {
        format!("{}/test", self.api)
    }
}

#[cfg(test)]
mod tests {
    use super::Urls;
    use crate::operation::ops;

    const API: &str = "http://h:1/api/v3";
    const WEB: &str = "http://h:1";

    fn urls() -> Urls<'static> {
        Urls::new(API, WEB)
    }

    #[test]
    #[should_panic(expected = "no value for {repo} in /repos/{owner}/{repo}")]
    fn a_missing_parameter_panics() {
        super::api_url::<ops::ReposGet>(API, &[("owner", &"o")]);
    }

    #[test]
    #[should_panic(expected = "unused parameters for /repos/{owner}/{repo}")]
    fn an_unused_parameter_panics() {
        super::api_url::<ops::ReposGet>(API, &[("owner", &"o"), ("repo", &"r"), ("x", &1)]);
    }

    #[test]
    fn multi_segment_parameters_keep_their_slashes() {
        assert_eq!(
            super::api_url::<ops::ReposGetBranch>(
                API,
                &[("owner", &"o"), ("repo", &"r"), ("branch", &"a/b c")]
            ),
            "http://h:1/api/v3/repos/o/r/branches/a/b c"
        );
        assert_eq!(
            super::api_url::<ops::IssuesGetLabel>(
                API,
                &[("owner", &"o"), ("repo", &"r"), ("name", &"a/b c")]
            ),
            "http://h:1/api/v3/repos/o/r/labels/a%2Fb%20c"
        );
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

    #[test]
    fn every_other_location() {
        let urls = urls();
        assert_eq!(urls.api("/user"), "http://h:1/api/v3/user");
        assert_eq!(urls.web("/settings"), "http://h:1/settings");
        let org = urls.org("acme");
        assert_eq!(org.html, "http://h:1/acme");
        assert_eq!(org.avatar, "http://h:1/avatars/acme");
        assert_eq!(org.api("/repos"), "http://h:1/api/v3/orgs/acme/repos");
        assert_eq!(
            org.membership("bob"),
            "http://h:1/api/v3/orgs/acme/memberships/bob"
        );
        assert_eq!(
            org.public_member("bob"),
            "http://h:1/api/v3/orgs/acme/public_members/bob"
        );
        assert_eq!(
            org.runner_group_runners(3),
            "http://h:1/api/v3/orgs/acme/actions/runner-groups/3/runners"
        );
        assert_eq!(
            org.runner_group_repositories(3),
            "http://h:1/api/v3/orgs/acme/actions/runner-groups/3/repositories"
        );
        let team = urls.team("acme", "core");
        assert_eq!(
            team.repositories(),
            "http://h:1/api/v3/orgs/acme/teams/core/repos"
        );
        assert_eq!(
            team.membership("bob"),
            "http://h:1/api/v3/orgs/acme/teams/core/memberships/bob"
        );

        let gist = Urls::new("http://h:1/api/v3/", "http://h:1/").gist("aa1");
        assert_eq!(gist.api, "http://h:1/api/v3/gists/aa1");
        assert_eq!(gist.html, "http://h:1/gist/aa1");
        assert_eq!(gist.comments(), "http://h:1/api/v3/gists/aa1/comments");
        assert_eq!(gist.commits(), "http://h:1/api/v3/gists/aa1/commits");
        assert_eq!(gist.forks(), "http://h:1/api/v3/gists/aa1/forks");
        assert_eq!(gist.raw("a b.txt"), "http://h:1/raw/aa1/a b.txt");
        assert_eq!(gist.git(), "https://gist.github.test/aa1.git");

        let installation = urls.installation(8);
        assert_eq!(
            installation.access_tokens,
            "http://h:1/api/v3/app/installations/8/access_tokens"
        );
        assert_eq!(installation.html, "http://h:1/settings/installations/8");
        assert_eq!(urls.app_html("bot"), "http://h:1/apps/bot");
        assert_eq!(
            urls.job_logs("o", "r", 2),
            "http://h:1/_ninot/actions/logs/o/r/2"
        );
        assert_eq!(
            urls.run_log_archive("o", "r", 2),
            "http://h:1/_ninot/actions/log-archives/o/r/2"
        );
        assert_eq!(super::enterprise_api("http://h:1"), "http://h:1/api/v3");
        assert_eq!(super::graphql("http://h:1"), "http://h:1/api/graphql");
        assert_eq!(
            super::service("http://h:1", "run/"),
            "http://h:1/_services/run/"
        );
    }

    #[test]
    fn every_repository_resource() {
        let repo = urls().repo("o", "r");
        assert_eq!(repo.api("/forks"), "http://h:1/api/v3/repos/o/r/forks");
        let pull = repo.pull(7);
        assert_eq!(pull.patch(), "http://h:1/o/r/pull/7.patch");
        assert_eq!(
            pull.review_comment_html(4),
            "http://h:1/o/r/pull/7#discussion_r4"
        );
        assert_eq!(pull.comment_html(5), "http://h:1/o/r/pull/7#issuecomment-5");
        assert_eq!(pull.issue.html, "http://h:1/o/r/issues/7");
        assert_eq!(
            pull.issue.comments(),
            "http://h:1/api/v3/repos/o/r/issues/7/comments"
        );
        assert_eq!(
            pull.issue.events(),
            "http://h:1/api/v3/repos/o/r/issues/7/events"
        );
        assert_eq!(
            repo.issue_comment(9),
            "http://h:1/api/v3/repos/o/r/issues/comments/9"
        );
        assert_eq!(
            repo.issue_event(9),
            "http://h:1/api/v3/repos/o/r/issues/events/9"
        );
        assert_eq!(
            repo.review_comment(9),
            "http://h:1/api/v3/repos/o/r/pulls/comments/9"
        );
        assert_eq!(
            repo.review_comment_template(),
            "http://h:1/api/v3/repos/o/r/pulls/comments{/number}"
        );
        let commit = repo.commit("abc");
        assert_eq!(
            commit.comments(),
            "http://h:1/api/v3/repos/o/r/commits/abc/comments"
        );
        assert_eq!(
            repo.statuses("abc"),
            "http://h:1/api/v3/repos/o/r/statuses/abc"
        );
        assert_eq!(
            repo.status("abc", 3),
            "http://h:1/api/v3/repos/o/r/statuses/abc"
        );
        for collection in ["commits", "trees", "blobs", "tags"] {
            assert_eq!(
                repo.git_object(collection, "abc"),
                format!("http://h:1/api/v3/repos/o/r/git/{collection}/abc")
            );
        }
        assert_eq!(
            repo.git_ref("refs/tags/v1/rc"),
            "http://h:1/api/v3/repos/o/r/git/refs/tags/v1/rc"
        );
        assert_eq!(repo.tree_html("abc"), "http://h:1/o/r/tree/abc");
        assert_eq!(
            repo.entry_html("tree", "main", "a/b"),
            "http://h:1/o/r/tree/main/a/b"
        );
        assert_eq!(
            repo.contents("a b/c+d.txt", "feature/x"),
            "http://h:1/api/v3/repos/o/r/contents/a b/c+d.txt?ref=feature/x"
        );
        assert_eq!(
            repo.branch_protection("a/b"),
            "http://h:1/api/v3/repos/o/r/branches/a/b/protection"
        );
        let compare = repo.compare("main", "feature/x");
        assert_eq!(
            compare.api,
            "http://h:1/api/v3/repos/o/r/compare/main...feature/x"
        );
        assert_eq!(compare.html, "http://h:1/o/r/compare/main...feature/x");
        assert_eq!(
            compare.diff(),
            "http://h:1/o/r/compare/main...feature/x.diff"
        );
        assert_eq!(
            repo.tarball("v1/rc"),
            "http://h:1/api/v3/repos/o/r/tarball/v1/rc"
        );
        assert_eq!(
            repo.zipball("v1/rc"),
            "http://h:1/api/v3/repos/o/r/zipball/v1/rc"
        );
        let milestone = repo.milestone(2);
        assert_eq!(milestone.api, "http://h:1/api/v3/repos/o/r/milestones/2");
        assert_eq!(milestone.html, "http://h:1/o/r/milestone/2");
        assert_eq!(
            milestone.labels(),
            "http://h:1/api/v3/repos/o/r/milestones/2/labels"
        );
        assert_eq!(
            repo.check_run(5).api,
            "http://h:1/api/v3/repos/o/r/check-runs/5"
        );
        assert_eq!(
            repo.check_suite(6).api,
            "http://h:1/api/v3/repos/o/r/check-suites/6"
        );
        assert_eq!(
            repo.workflow("ci.yml"),
            "http://h:1/api/v3/repos/o/r/actions/workflows/ci.yml"
        );
        assert_eq!(
            repo.workflow_badge(3),
            "http://h:1/o/r/actions/workflows/3/badge.svg"
        );
        let run = repo.workflow_run(4);
        assert_eq!(run.api, "http://h:1/api/v3/repos/o/r/actions/runs/4");
        assert_eq!(run.html, "http://h:1/o/r/actions/runs/4");
        assert_eq!(
            run.api("/jobs"),
            "http://h:1/api/v3/repos/o/r/actions/runs/4/jobs"
        );
        let release = repo.release(4, "v1/rc");
        assert_eq!(release.api, "http://h:1/api/v3/repos/o/r/releases/4");
        assert_eq!(release.html, "http://h:1/o/r/releases/tag/v1/rc");
        assert_eq!(
            release.assets(),
            "http://h:1/api/v3/repos/o/r/releases/4/assets"
        );
        assert_eq!(
            repo.release_asset(9),
            "http://h:1/api/v3/repos/o/r/releases/assets/9"
        );
        assert_eq!(
            repo.release_download("v1", "a b.zip"),
            "http://h:1/o/r/releases/download/v1/a b.zip"
        );
        let hook = repo.hook(3);
        assert_eq!(hook.api, "http://h:1/api/v3/repos/o/r/hooks/3");
        assert_eq!(
            hook.deliveries(),
            "http://h:1/api/v3/repos/o/r/hooks/3/deliveries"
        );
        assert_eq!(hook.pings(), "http://h:1/api/v3/repos/o/r/hooks/3/pings");
        assert_eq!(hook.test(), "http://h:1/api/v3/repos/o/r/hooks/3/test");
    }
}
