pub mod execute_script;

use crate::{config::Macro, rocket_types::*, sql::*, ManagedConfig, ManagedPool};
use rocket::{http::ContentType, serde::json::Json, State};
use serde::Serialize;
use tokio::fs;

#[derive(Debug, Serialize)]
pub struct ApiEmail {
    from_addr: String,
    to_addr: String,
    subject: String,
    id: String,
    registered: i64,
}
impl From<Email> for ApiEmail {
    fn from(email: Email) -> Self {
        ApiEmail {
            from_addr: email.from_addr,
            to_addr: email.to_addr,
            subject: email.subject,
            id: email.id,
            registered: email.registered,
        }
    }
}

#[rocket::get("/emails/list")]
pub async fn list_emails(
    user: AuthorizedUser<'_>,
    pool: &State<ManagedPool>,
    _ratelimit: Ratelimit,
) -> Result<FlexibleFormat<ApiEmail>, Error> {
    let user_emails: Vec<Email> = match sqlx::query_as!(
        Email,
        r#"SELECT * FROM emails WHERE user = $1 ORDER BY registered DESC"#,
        user.username
    )
    .fetch_all(&**pool)
    .await
    {
        Ok(x) => x,
        Err(e) => {
            eprintln!("/emails/list SELECT error: {:#?}", e);
            return Err(Error::InternalError);
        }
    };

    Ok(FlexibleFormat::from_vec(
        user_emails.into_iter().map(ApiEmail::from).collect(),
    ))
}

#[rocket::get("/emails/<id>/html")]
pub async fn view_email(
    id: &str,
    user: AuthorizedUser<'_>,
    pool: &State<ManagedPool>,
    config: &State<ManagedConfig>,
    _ratelimit: Ratelimit,
) -> Result<(ContentType, Vec<u8>), Error> {
    let email = match sqlx::query_as!(
        Email,
        r#"SELECT * FROM emails WHERE id = $1 AND user = $2"#,
        id,
        user.username
    )
    .fetch_optional(&**pool)
    .await
    {
        Ok(Some(email)) => email,
        Ok(None) => return Err(Error::Unauthorized),
        Err(e) => {
            eprintln!("/emails/<id>/html SELECT error: {:#?}", e);
            return Err(Error::InternalError);
        }
    };

    match fs::read(format!("{}/{}", config.storage.file_root, email.html)).await {
        Ok(bytes) => Ok((ContentType::HTML, bytes)),
        Err(e) => {
            eprintln!("/emails/<id>/html fs::read error: {:#?}", e);
            return Err(Error::InternalError);
        }
    }
}

#[rocket::get("/emails/<id>")]
pub async fn get_email(
    id: &str,
    user: AuthorizedUser<'_>,
    pool: &State<ManagedPool>,
    _ratelimit: Ratelimit,
) -> Result<Json<ApiEmail>, Error> {
    let email = match sqlx::query_as!(
        Email,
        r#"SELECT * FROM emails WHERE user = $1 AND id = $2"#,
        user.username,
        id
    )
    .fetch_one(&**pool)
    .await
    {
        Ok(x) => x,
        Err(e) => {
            eprintln!("/emails/<id> SELECT error: {:#?}", e);
            return Err(Error::InternalError);
        }
    };

    Ok(Json(email.into()))
}

#[rocket::get("/macros/list")]
pub async fn list_macros<'a>(
    _user: AuthorizedUser<'_>,
    config: &'a State<ManagedConfig>,
    _ratelimit: Ratelimit,
) -> FlexibleFormat<&'a str> {
    FlexibleFormat::from_vec(config.macros.iter().map(|mac| &*mac.name).collect())
}

#[rocket::get("/macros/<name>")]
pub async fn get_macro<'a>(
    name: String,
    _user: AuthorizedUser<'_>,
    config: &'a State<ManagedConfig>,
    _ratelimit: Ratelimit,
) -> Result<Json<&'a Macro>, Error> {
    if let Some(mac) = config.macros.iter().find(|mac| mac.name == name) {
        Ok(Json(mac))
    } else {
        Err(Error::NotFound)
    }
}

#[derive(Debug, Serialize)]
pub struct Verified {
    verified: bool,
}

#[rocket::get("/auth/verify")]
pub async fn verify_auth(_user: AuthorizedUser<'_>, _ratelimit: Ratelimit) -> Json<Verified> {
    Json(Verified { verified: true })
}
