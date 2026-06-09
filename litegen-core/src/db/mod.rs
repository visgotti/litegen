mod postgres;
pub mod sqlite;
mod trait_def;
#[cfg(test)] mod sqlite_tests;
#[cfg(test)] mod postgres_tests;

pub use postgres::PostgresDatabase;
pub use sqlite::SqliteDatabase;
pub use trait_def::DatabaseStore;

/// Connect to the appropriate database backend based on the URL scheme.
///
/// - URLs starting with `postgres://` or `postgresql://` use PostgreSQL.
/// - Everything else (including `sqlite://`) uses SQLite.
pub async fn connect(database_url: &str) -> Result<Box<dyn DatabaseStore>, sqlx::Error> {
    if database_url.starts_with("postgres://") || database_url.starts_with("postgresql://") {
        let db = PostgresDatabase::connect(database_url).await?;
        Ok(Box::new(db))
    } else {
        let db = SqliteDatabase::connect(database_url).await?;
        Ok(Box::new(db))
    }
}
