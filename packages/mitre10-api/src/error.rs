//! What Mitre 10 said no with.
//!
//! Two shapes are particular to this storefront. The API's CORS rules answer
//! `403` with a body of `Invalid CORS request` when `Origin` is wrong or a
//! session has moved hosts, which is a client bug rather than a credential
//! one -- so it gets its own variant. And OCC reports business failures as a
//! `400` carrying `errors[]`, so a refused voucher and a transport error
//! arrive by different routes and both end up here.

use net_kit::{AuthFault, Fault, HttpError};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Http(#[from] HttpError),

    #[error(transparent)]
    Net(#[from] net_kit::Error),

    /// The site asked this client to slow down. Its own variant because the
    /// right response is to stop, not to retry harder.
    #[error("Mitre 10 is rate-limiting this client{}", match .retry_after {
        Some(secs) => format!("; it asked for {secs}s before the next request"),
        None => String::new(),
    })]
    RateLimited { retry_after: Option<u64> },

    /// OCC answered with an `errors[]` entry. The operation is carried because
    /// the message alone rarely says which call produced it.
    #[error("{operation} was refused: {message}")]
    Occ {
        operation: &'static str,
        message: String,
    },

    /// `403 Invalid CORS request`. Not a credential problem: the request was
    /// missing or contradicting its `Origin`, or the session cookie was minted
    /// against a different one.
    #[error("Mitre 10 refused the request's origin; the session may have been started elsewhere")]
    Cors,

    /// The access token has expired. Recoverable from the refresh token
    /// without a password -- see [`crate::auth`].
    #[error("the Mitre 10 session has expired")]
    SessionExpired,

    #[error("not signed in to Mitre 10")]
    NotSignedIn,

    /// The refresh token was refused, which is the one auth failure a refresh
    /// cannot fix.
    #[error("the stored Mitre 10 login has lapsed and a new sign-in is needed")]
    LoginLapsed,

    /// The sign-in form rejected the username or password.
    #[error("Mitre 10 rejected those credentials")]
    BadCredentials,

    /// `/authorizationserver/csrf` refused this client.
    ///
    /// Its own variant because it arrives as `403 Invalid CORS request` and is
    /// neither a CORS bug nor a credential one: the endpoint wants a session
    /// the storefront's own pages establish, and a cold client has none. Read
    /// as a plain 403 it sends someone to re-enter a password that was never
    /// wrong.
    #[error("Mitre 10's sign-in endpoint refused a client with no storefront session")]
    LoginUnavailable,

    #[error("no product called {0}")]
    NoSuchProduct(String),

    #[error("no store called {0}")]
    NoSuchStore(String),

    #[error("no category called {0}")]
    NoSuchCategory(String),

    /// The answer parsed but did not contain what was asked for. Named apart
    /// from a decode failure: the schema moved, the JSON was not malformed.
    #[error("{0}")]
    Shape(String),

    #[error("{context}")]
    Decode {
        context: String,
        #[source]
        source: serde_json::Error,
    },
}

impl Error {
    pub fn decode(context: impl Into<String>, source: serde_json::Error) -> Error {
        Error::Decode {
            context: context.into(),
            source,
        }
    }

    pub fn body(&self) -> &str {
        match self {
            Error::Http(e) => e.body(),
            _ => "",
        }
    }

    pub fn is_rate_limited(&self) -> bool {
        matches!(self, Error::RateLimited { .. })
    }

    /// Whether a client holding a refresh token should mint a new access token
    /// and try again. Excludes [`Error::LoginLapsed`]: that one has already
    /// tried and failed, and retrying it loops.
    pub fn is_lapsed(&self) -> bool {
        matches!(self, Error::SessionExpired | Error::NotSignedIn)
    }

    /// Whether the only way forward is a fresh sign-in with a password.
    pub fn needs_login(&self) -> bool {
        matches!(self, Error::LoginLapsed | Error::BadCredentials)
    }

    /// Whether signing in cannot currently be attempted at all -- as opposed
    /// to having been attempted and refused.
    pub fn is_login_unavailable(&self) -> bool {
        matches!(self, Error::LoginUnavailable)
    }

    /// Classify a status the storefront returned on an authenticated call.
    pub fn from_status(status: u16, body: &str) -> Option<Error> {
        match status {
            401 => Some(Error::SessionExpired),
            403 if body.contains("Invalid CORS request") => Some(Error::Cors),
            429 => Some(Error::RateLimited { retry_after: None }),
            _ => None,
        }
    }
}

impl Fault for Error {
    fn auth(&self) -> Option<AuthFault> {
        match self {
            Error::SessionExpired => Some(AuthFault::Expired),
            Error::NotSignedIn => Some(AuthFault::Missing),
            Error::LoginLapsed | Error::BadCredentials => Some(AuthFault::Rejected),
            // Not Rejected: nothing was rejected, the attempt never happened.
            Error::LoginUnavailable => None,
            Error::Http(e) => e.auth(),
            _ => None,
        }
    }

    fn is_transport(&self) -> bool {
        matches!(self, Error::Http(e) if e.is_transport())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_expired_token_is_refreshable_and_a_lapsed_login_is_not() {
        // `is_lapsed` gates an automatic retry, so a lapsed login answering
        // true would loop: refresh, fail, refresh again.
        assert!(Error::SessionExpired.is_lapsed());
        assert!(!Error::SessionExpired.needs_login());
        assert!(!Error::LoginLapsed.is_lapsed());
        assert!(Error::LoginLapsed.needs_login());
    }

    #[test]
    fn a_cors_refusal_is_not_read_as_a_credential_problem() {
        // Both arrive as a 403. Only one is fixed by signing in again, and
        // sending someone to retype a password that was never wrong is worse
        // than saying nothing.
        let cors = Error::from_status(403, "Invalid CORS request").expect("classified");
        assert!(matches!(cors, Error::Cors));
        assert_eq!(cors.auth(), None);
        assert!(!cors.is_lapsed());

        assert!(matches!(
            Error::from_status(401, "").expect("classified"),
            Error::SessionExpired
        ));
        assert!(Error::from_status(403, "Forbidden").is_none());
    }

    #[test]
    fn an_unavailable_sign_in_is_not_reported_as_bad_credentials() {
        // It arrives as a 403 alongside genuine auth failures. Treating it as
        // one would have someone retyping a correct password.
        let e = Error::LoginUnavailable;
        assert!(e.is_login_unavailable());
        assert!(!e.needs_login(), "there is no password to fix this");
        assert!(!e.is_lapsed(), "and nothing to refresh");
        assert_eq!(e.auth(), None);
    }

    #[test]
    fn a_rate_limit_is_named_rather_than_left_as_a_status_code() {
        let limited = Error::RateLimited {
            retry_after: Some(30),
        };
        assert!(limited.is_rate_limited());
        assert!(limited.to_string().contains("rate-limiting"), "{limited}");
        assert!(limited.to_string().contains("30s"), "{limited}");
        assert_eq!(limited.auth(), None);
    }

    #[test]
    fn an_occ_refusal_names_the_operation_it_came_from() {
        let e = Error::Occ {
            operation: "addToCart",
            message: "Product is not available".into(),
        };
        assert!(e.to_string().contains("addToCart"), "{e}");
    }
}
