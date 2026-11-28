//! Paradis server binary entrypoint.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::{env, error::Error, path::Path};

use paradis_server::{app, config::Config, db};
use tokio::net::TcpListener;

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let environment = app_environment()?;
    let config_path = Path::new("server/config").join(format!("config_{environment}.yaml"));
    let config = Config::load(&config_path)
        .map_err(|e| format!("load config {}: {e}", config_path.display()))?;

    let pool = db::connect(&config.sqlite.dsn)
        .await
        .map_err(|e| format!("open SQLite database {}: {e}", config.sqlite.dsn))?;
    db::migrate(&pool)
        .await
        .map_err(|e| format!("apply database migrations: {e}"))?;

    let addr = env::var("LISTEN_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_owned());
    let listener = TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("bind {addr}: {e}"))?;
    println!(
        "paradis-server ({environment}) listening on http://{addr}; database {}",
        config.sqlite.dsn
    );

    axum::serve(listener, app(pool.clone())).await?;
    pool.close().await;
    Ok(())
}

/// `APP_ENVIRONMENT`, lower-cased; `dev` when unset or empty.
fn app_environment() -> Result<String, String> {
    let environment = env::var("APP_ENVIRONMENT")
        .ok()
        .map(|value| value.trim().to_ascii_lowercase())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "dev".to_owned());

    if !environment
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    {
        return Err(format!(
            "APP_ENVIRONMENT {environment:?} may only contain letters, digits, '-' and '_'"
        ));
    }
    Ok(environment)
}
