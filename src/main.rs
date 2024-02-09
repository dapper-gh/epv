mod api;
mod config;
mod error_handling;
mod imap;
mod rocket_types;
mod sql;
mod util;

use std::net::IpAddr;
use std::sync::Arc;

use tokio::time::Instant;

use rocket::{
    fs::{FileServer, Options as FsOptions},
    Config as RocketConfig,
};
use sqlx::{Pool, Sqlite};

use sqlx::sqlite::SqlitePoolOptions;

use dashmap::DashMap;

use url::Url;

use config::Config;
use util::Cache;

pub type ManagedConfig = Arc<Config>;
pub type ManagedPool = Pool<Sqlite>;
pub type ManagedRatelimits = Arc<DashMap<IpAddr, Vec<Instant>>>;
pub type ManagedUrlCache = Cache<Url, Url, 1000>;

#[tokio::main]
async fn main() {
    let config = Arc::new(config::load_config().await);
    let ratelimits: ManagedRatelimits = Arc::new(DashMap::new());
    let url_cache = ManagedUrlCache::new();

    let pool = SqlitePoolOptions::new()
        .max_connections(32)
        .min_connections(1)
        .connect(&config.storage.sqlite)
        .await
        .expect("Unable to connect to DB");

    let config_imap = Arc::clone(&config);
    let pool_imap = pool.clone();
    tokio::spawn(imap::perform(config_imap, pool_imap));

    rocket::custom(
        RocketConfig::figment()
            .merge(("port", 57331))
            .merge(("ident", false))
            .merge(("cli_colors", false)),
    )
    .manage(Arc::clone(&config))
    .manage(pool)
    .manage(ratelimits)
    .manage(url_cache)
    .mount(
        "/api",
        rocket::routes![
            api::list_emails,
            api::view_email,
            api::execute_script::execute_script,
            api::list_macros,
            api::get_macro,
            api::verify_auth,
            api::get_email
        ],
    )
    .mount(
        "/",
        FileServer::new(
            config.storage.frontend.to_string(),
            FsOptions::Index | FsOptions::NormalizeDirs,
        ),
    )
    .register(
        "/",
        rocket::catchers![
            error_handling::unauthorized,
            error_handling::internal_server_error,
            error_handling::not_found,
            error_handling::too_many_requests
        ],
    )
    .launch()
    .await
    .expect("Failed to launch Rocket");
}
