use crate::rocket_types::Error;
use rocket::Request;

#[rocket::catch(401)]
pub async fn unauthorized(_req: &Request<'_>) -> Error {
    Error::Unauthorized
}

#[rocket::catch(500)]
pub async fn internal_server_error(_req: &Request<'_>) -> Error {
    Error::InternalError
}

#[rocket::catch(404)]
pub async fn not_found(_req: &Request<'_>) -> Error {
    Error::NotFound
}

#[rocket::catch(429)]
pub async fn too_many_requests(_req: &Request<'_>) -> Error {
    Error::Ratelimited
}
