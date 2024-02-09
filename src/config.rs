use serde::{Deserialize, Serialize};

use tokio::fs;

#[derive(Deserialize, Clone, Debug)]
pub struct Config {
    pub users: Users,
    pub imap: Imap,
    pub storage: Storage,
    pub macros: Vec<Macro>,
    pub ratelimit: Ratelimit,
}

#[derive(Deserialize, Clone, Debug)]
#[serde(untagged)]
pub enum Users {
    Single(User),
    Many(Vec<User>),
}

#[derive(Deserialize, Clone, Debug)]
pub struct User {
    pub username: String,
    pub password: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Imap {
    pub server: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub postfix: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Storage {
    pub file_root: String,
    pub sqlite: String,
    pub frontend: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct Ratelimit {
    pub num: usize,
    pub in_ms: u128,
}

#[derive(Deserialize, Clone, Debug, Serialize)]
pub struct Macro {
    pub name: String,
    pub actions: Vec<crate::api::execute_script::Action>,
}

pub async fn load_config() -> Config {
    let bytes = fs::read("config.json")
        .await
        .expect("Could not read config.json");
    serde_json::from_slice(&bytes).expect("Could not parse config.json")
}
