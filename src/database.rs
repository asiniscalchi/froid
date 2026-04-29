use sqlite_vec::sqlite3_vec_init;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
};

type SqliteExtensionFn = unsafe extern "C" fn(
    *mut libsqlite3_sys::sqlite3,
    *mut *mut i8,
    *const libsqlite3_sys::sqlite3_api_routines,
) -> i32;

pub fn register_sqlite_vec_extension() {
    unsafe {
        libsqlite3_sys::sqlite3_auto_extension(Some(std::mem::transmute::<
            *const (),
            SqliteExtensionFn,
        >(sqlite3_vec_init as *const ())));
    }
}

pub async fn connect_pool(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    register_sqlite_vec_extension();

    let options = sqlite_connect_options(database_url)?;

    SqlitePoolOptions::new().connect_with(options).await
}

fn sqlite_connect_options(database_url: &str) -> Result<SqliteConnectOptions, sqlx::Error> {
    Ok(database_url
        .parse::<SqliteConnectOptions>()?
        .create_if_missing(true))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    #[tokio::test]
    async fn connect_pool_creates_missing_database_file() {
        let database_path = std::env::temp_dir().join(format!(
            "froid-test-{}.sqlite3",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let database_url = format!("sqlite:{}", database_path.display());

        let pool = connect_pool(&database_url).await.unwrap();
        pool.close().await;

        assert!(database_path.exists());

        fs::remove_file(database_path).unwrap();
    }
}
