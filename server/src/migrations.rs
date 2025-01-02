use anyhow::Context;
use diesel::pg::Pg;
use diesel_async::async_connection_wrapper::AsyncConnectionWrapper;
use diesel_async::AsyncConnection;
use diesel_migrations::{embed_migrations, EmbeddedMigrations, MigrationHarness};

const MIGRATIONS: EmbeddedMigrations = embed_migrations!();

pub async fn run_migrations<A>(async_connection: A) -> anyhow::Result<()>
where
    A: AsyncConnection<Backend = Pg> + 'static,
{
    let mut async_wrapper: AsyncConnectionWrapper<A> = AsyncConnectionWrapper::from(async_connection);

    tokio::task::spawn_blocking(move || {
        async_wrapper
            .run_pending_migrations(MIGRATIONS)
            .map_err(|e| anyhow::anyhow!(e))?;
        anyhow::Ok(())
    })
    .await
    .context("thread panicked")??;

    Ok(())
}
