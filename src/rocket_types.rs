use crate::{
    config::{User, Users},
    ManagedConfig, ManagedRatelimits,
};
use csv::{QuoteStyle, WriterBuilder};
use rocket::http::ContentType;
use rocket::{
    http::Status,
    request::{FromRequest, Outcome, Request},
    response::Responder,
    serde::json::Json,
    State,
};
use serde::Serialize;
use std::ops::Deref;
use tokio::time::Instant;

#[derive(Debug, Serialize)]
#[serde(tag = "error", content = "data")]
pub enum Error {
    InternalError,
    Unauthorized,
    InvalidInput(String),
    NotFound,
    Ratelimited,
}

impl<'r, 'o: 'r> Responder<'r, 'o> for Error {
    fn respond_to(self, request: &'r Request<'_>) -> rocket::response::Result<'o> {
        match self {
            Error::InternalError => (Status::InternalServerError, Json(self)).respond_to(request),
            Error::Unauthorized => (Status::Unauthorized, Json(self)).respond_to(request),
            Error::InvalidInput(_) => (Status::BadRequest, Json(self)).respond_to(request),
            Error::NotFound => (Status::NotFound, Json(self)).respond_to(request),
            Error::Ratelimited => (Status::TooManyRequests, Json(self)).respond_to(request),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ExpectedFormat {
    Json,
    Csv,
}
#[rocket::async_trait]
impl<'r> FromRequest<'r> for ExpectedFormat {
    type Error = Error;

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        Outcome::Success(Self::from_request_sync(request))
    }
}
impl ExpectedFormat {
    pub fn from_request_sync(request: &Request) -> Self {
        if Some("csv")
            == request.uri().query().and_then(|query| {
                query
                    .segments()
                    .find_map(|(key, value)| if key == "format" { Some(value) } else { None })
            })
        {
            ExpectedFormat::Csv
        } else {
            ExpectedFormat::Json
        }
    }
}

pub struct FlexibleFormatComplex<T, F> {
    data: T,
    processor: F,
}

enum FlexibleFormatInner<T, V, F> {
    Complex(FlexibleFormatComplex<T, F>),
    Vec(Vec<V>),
}
pub struct FlexibleFormat<T, V = T, F = fn(T) -> Vec<V>> {
    inner: FlexibleFormatInner<T, V, F>,
    include_header: bool,
}
impl<'r, 'o: 'r, T: Serialize, V: Serialize, F: FnOnce(T) -> Vec<V>> Responder<'r, 'o>
    for FlexibleFormat<T, V, F>
{
    fn respond_to(self, request: &'r Request<'_>) -> rocket::response::Result<'o> {
        let expected_format = ExpectedFormat::from_request_sync(request);
        match (self.inner, expected_format) {
            (FlexibleFormatInner::Complex(inner), ExpectedFormat::Json) => {
                Json(inner.data).respond_to(request)
            }
            (FlexibleFormatInner::Vec(v), ExpectedFormat::Json) => Json(v).respond_to(request),
            (FlexibleFormatInner::Vec(v), ExpectedFormat::Csv) => {
                let mut writer = WriterBuilder::new()
                    .has_headers(self.include_header)
                    .quote_style(QuoteStyle::Always)
                    .from_writer(vec![]);

                for item in v {
                    if let Err(e) = writer.serialize(item) {
                        eprintln!("CSV writer error: {:#?}", e);
                        return Err(Status::InternalServerError);
                    }
                }

                let bytes = match writer.into_inner() {
                    Ok(x) => x,
                    Err(e) => {
                        eprintln!("CSV inner error: {:#?}", e);
                        return Err(Status::InternalServerError);
                    }
                };

                (ContentType::CSV, bytes).respond_to(request)
            }
            (FlexibleFormatInner::Complex(inner), ExpectedFormat::Csv) => {
                (FlexibleFormat::<u8, V> {
                    inner: FlexibleFormatInner::Vec((inner.processor)(inner.data)),
                    include_header: self.include_header,
                })
                .respond_to(request)
            }
        }
    }
}
impl<T, V, F: FnOnce(T) -> Vec<V>> FlexibleFormat<T, V, F> {
    fn from_inner(inner: FlexibleFormatInner<T, V, F>) -> Self {
        FlexibleFormat {
            inner,
            include_header: true,
        }
    }

    pub fn from_vec(v: Vec<V>) -> Self {
        Self::from_inner(FlexibleFormatInner::Vec(v))
    }

    pub fn from_complex(data: T, processor: F) -> Self {
        Self::from_inner(FlexibleFormatInner::Complex(FlexibleFormatComplex {
            data,
            processor,
        }))
    }

    pub fn include_header(&mut self, new_value: bool) -> &mut Self {
        self.include_header = new_value;
        self
    }
}

#[derive(Debug)]
pub struct AuthorizedUser<'a> {
    pub user: &'a User,
}

impl<'a> Deref for AuthorizedUser<'a> {
    type Target = User;

    fn deref(&self) -> &Self::Target {
        self.user
    }
}

#[rocket::async_trait]
impl<'r> FromRequest<'r> for AuthorizedUser<'r> {
    type Error = Error;

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        let Some(auth) = request.headers().get_one("Authorization").or_else(|| {
            request.uri().query().and_then(|query| {
                query
                    .segments()
                    .find_map(|(key, value)| if key == "auth" { Some(value) } else { None })
            })
        }) else {
            return Outcome::Error((Status::Unauthorized, Error::Unauthorized));
        };

        let Some((username, password)) = auth.split_once(':') else {
            return Outcome::Error((Status::Unauthorized, Error::Unauthorized));
        };

        let config: &State<ManagedConfig> = match request.guard().await {
            Outcome::Success(state) => state,
            _ => return Outcome::Error((Status::Unauthorized, Error::Unauthorized)),
        };

        if let Some(user) = match &config.users {
            Users::Many(users) => users
                .iter()
                .find(|user| user.username == username && user.password == password),
            Users::Single(user) => {
                if user.username == username && user.password == password {
                    Some(user)
                } else {
                    None
                }
            }
        } {
            Outcome::Success(AuthorizedUser { user })
        } else {
            Outcome::Error((Status::Unauthorized, Error::Unauthorized))
        }
    }
}

#[derive(Debug)]
pub struct Ratelimit;

#[rocket::async_trait]
impl<'r> FromRequest<'r> for Ratelimit {
    type Error = Error;

    async fn from_request(request: &'r Request<'_>) -> Outcome<Self, Self::Error> {
        let ratelimits: &State<ManagedRatelimits> = match request.guard().await {
            Outcome::Success(x) => x,
            other => {
                eprintln!(
                    "Ratelimit from_request ManagedRatelimits error: {:#?}",
                    other
                );
                return Outcome::Error((Status::InternalServerError, Error::InternalError));
            }
        };

        let config: &State<ManagedConfig> = match request.guard().await {
            Outcome::Success(x) => x,
            other => {
                eprintln!("Ratelimit from_request ManagedConfig error: {:#?}", other);
                return Outcome::Error((Status::InternalServerError, Error::InternalError));
            }
        };

        let Some(ip) = request.client_ip() else {
            eprintln!("Ratelimit from_request .client_ip() None");
            return Outcome::Error((Status::InternalServerError, Error::InternalError));
        };

        let mut previous_requests = ratelimits
            .entry(ip)
            .or_insert_with(|| Vec::with_capacity(config.ratelimit.num));
        *previous_requests = previous_requests
            .iter()
            .filter(|instant| instant.elapsed().as_millis() < config.ratelimit.in_ms)
            .copied()
            .collect();
        if previous_requests.len() >= config.ratelimit.num {
            Outcome::Error((Status::TooManyRequests, Error::Ratelimited))
        } else {
            previous_requests.push(Instant::now());

            Outcome::Success(Ratelimit)
        }
    }
}
