//! Billing plans of the fixture accounts.

use super::*;

impl Org {
    /// Sets the organization's billing plan, which `GET /orgs/{org}` shows its owners and
    /// which limits rulesets and environment reviewers on its private repositories. `None`, the
    /// default, shows no plan and applies no limits.
    ///
    /// # Errors
    /// When the server has stopped.
    pub fn set_plan(&self, plan: Option<crate::state::Plan>) -> io::Result<()> {
        ensure_app_running(&self.app)?;
        let mut state = lock(&self.app.state);
        let org = state.account_mut(&self.login).ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "organization does not exist")
        })?;
        org.plan = plan;
        Ok(())
    }
}
