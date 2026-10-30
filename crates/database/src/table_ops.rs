//! Transactional submission of staged table changes.
use crate::{Connection, Error, Result, changes::TableChanges, connection::Pool};
use tokio_util::sync::CancellationToken;

impl Connection {
    /// Submit a complete edit batch atomically, rolling back on errors/cancellation.
    /// # Errors
    /// Rejects stale/missing identities and nontransactional MySQL tables.
    pub async fn apply_changes(
        &self,
        changes: &TableChanges,
        cancel: &CancellationToken,
    ) -> Result<()> {
        let statements = changes.statements()?;
        if changes.driver != self.driver {
            return Err(Error::Invalid(
                "Table dialect does not match connection".into(),
            ));
        }
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        if let Pool::Mysql(pool) = &self.pool {
            let engine: Option<String> = sqlx::query_scalar("SELECT CAST(ENGINE AS CHAR CHARACTER SET utf8mb4) FROM information_schema.TABLES WHERE TABLE_SCHEMA=? AND TABLE_NAME=?")
                .bind(&changes.schema).bind(&changes.table).fetch_optional(pool).await?.flatten();
            if !engine.is_some_and(|e| e.eq_ignore_ascii_case("InnoDB")) {
                return Err(Error::Invalid(
                    "Atomic Submit requires an InnoDB table".into(),
                ));
            }
        }
        macro_rules! prepare {
            ($connection:expr, sqlite) => {
                let token = cancel.clone();
                $connection
                    .lock_handle()
                    .await?
                    .set_progress_handler(1000, move || !token.is_cancelled());
            };
            ($connection:expr, server) => {
                $connection.close_on_drop();
            };
        }
        macro_rules! apply {
            ($pool:expr, $backend:ident) => {{
                let mut connection=tokio::select! { biased; ()=cancel.cancelled()=>return Err(Error::Cancelled), result=$pool.acquire()=>result? };
                prepare!(connection,$backend);
                let mut tx = sqlx::Acquire::begin(&mut *connection).await?;
                for sql in &statements {
                    if cancel.is_cancelled() { return Err(Error::Cancelled); }
                    let done=tokio::select! { biased; ()=cancel.cancelled()=>return Err(Error::Cancelled), result=sqlx::raw_sql(sql).execute(&mut *tx)=>result? };
                    let affected = done.rows_affected();
                    if affected != 1 {
                        return Err(Error::Invalid("Row changed or disappeared; batch rolled back. Reload before retrying.".into()));
                    }
                }
                if cancel.is_cancelled() { return Err(Error::Cancelled); }
                // Once COMMIT starts, await its outcome even if cancellation arrives.
                tx.commit().await?;
                Ok(())
            }};
        }
        match &self.pool {
            Pool::Postgres(p) => apply!(p, server),
            Pool::Mysql(p) => apply!(p, server),
            Pool::Sqlite(p) => apply!(p, sqlite),
        }
    }
}
