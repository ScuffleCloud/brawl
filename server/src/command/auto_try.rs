use anyhow::Context;
use diesel::OptionalExtension;
use diesel_async::{AsyncPgConnection, RunQueryDsl};

use super::BrawlCommandContext;
use crate::database::ci_run::CiRun;
use crate::database::pr::Pr;
use crate::github::messages;
use crate::github::models::PullRequest;
use crate::github::repo::GitHubRepoClient;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutoTryCommand {
    pub force: bool,
    pub disable: bool,
}

pub async fn handle<R: GitHubRepoClient>(
    conn: &mut AsyncPgConnection,
    context: BrawlCommandContext<'_, R>,
    command: AutoTryCommand,
) -> anyhow::Result<()> {
    let pr = context.repo.get_pull_request(context.pr_number).await?;
    handle_with_pr(conn, pr, context, command).await
}

async fn handle_with_pr<R: GitHubRepoClient>(
    conn: &mut AsyncPgConnection,
    pr: PullRequest,
    context: BrawlCommandContext<'_, R>,
    command: AutoTryCommand,
) -> anyhow::Result<()> {
    if !context.repo.config().enabled {
        return Ok(());
    }

    if !context.repo.can_try(context.user.id).await? {
        tracing::debug!("user does not have permission to do this");
        return Ok(());
    }

    let active_run = CiRun::active(context.repo.id(), context.pr_number)
        .get_result(conn)
        .await
        .optional()
        .context("fetch active run")?;

    if active_run.is_some_and(|r| !r.is_dry_run) {
        context.repo.send_message(context.pr_number, &messages::error_no_body("Cannot enable auto-try while a merge is in progress, use `?brawl cancel` to cancel it first & then try again.")).await?;
        return Ok(());
    }

    let db_pr = Pr::find(context.repo.id(), context.pr_number).get_result(conn).await?;

    match (command.disable, db_pr.auto_try) {
        (true, true) => {
            db_pr.update().auto_try(false).build().query().execute(conn).await?;

            context
                .repo
                .send_message(context.pr_number, &messages::auto_try_disabled())
                .await?;
        }
        (false, false) => {
            if !command.force && pr.head.repo.is_none_or(|r| r.id != context.repo.id()) {
                context.repo.send_message(context.pr_number, &messages::error_no_body("This PR is not from this repository, so auto-try cannot be enabled. To bypass this check, use force `?brawl auto-try force`")).await?;
                return Ok(());
            }

            db_pr.update().auto_try(true).build().query().execute(conn).await?;

            context
                .repo
                .send_message(context.pr_number, &messages::auto_try_enabled())
                .await?;
        }
        (_, _) => {}
    }

    Ok(())
}
