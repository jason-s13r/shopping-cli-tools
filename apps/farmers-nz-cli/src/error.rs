//! What `main` turns into an exit code.
//!
//! [`farmers_api::Error`] is the interesting half. This enum exists because the
//! app also has failures the retailer is not responsible for: an unreadable
//! config, a bad flag combination.

use farmers_api::Error as Api;

pub type AppResult<T> = std::result::Result<T, AppError>;

#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error(transparent)]
    Api(#[from] Api),

    #[error(transparent)]
    Net(#[from] net_kit::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Update(#[from] build_kit::Error),

    /// A flag combination no amount of network work would fix.
    #[error("{0}")]
    Usage(String),

    /// A failure the command has already described in full.
    ///
    /// `doctor` prints a report saying exactly what is wrong; adding
    /// "farmers: something is wrong" underneath says less than the report
    /// already did, but the exit code still has to carry.
    #[error("")]
    Reported(u8),
}

impl AppError {
    pub fn usage(message: impl Into<String>) -> AppError {
        AppError::Usage(message.into())
    }

    /// 2 is the shell's convention for misuse; 3 is an auth problem and 5 a
    /// missing thing, so a wrapper can tell "sign in again" from "that product
    /// does not exist" without reading the message.
    pub fn exit_code(&self) -> u8 {
        match self {
            AppError::Usage(_) => 2,
            AppError::Reported(code) => *code,
            AppError::Api(e) => match e {
                Api::SessionExpired | Api::NotSignedIn | Api::LoginRefused { .. } => 3,
                Api::NoSuchProduct(_) | Api::NoSuchCategory(_) | Api::NoSuchRegion(_) => 5,
                // Not 7, which every other tool here uses for a rate limit.
                // Nothing is being limited and waiting does not help, so a
                // script that backed off on this would sleep for nothing.
                Api::Denied { .. } | Api::Challenged => 8,
                _ => 1,
            },
            _ => 1,
        }
    }

    /// Whether `main` should print anything, or the command already did.
    pub fn silent(&self) -> bool {
        matches!(self, AppError::Reported(_))
    }
}

impl From<toml::de::Error> for AppError {
    fn from(e: toml::de::Error) -> AppError {
        AppError::Usage(format!("the config file is not valid TOML: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_exit_code_tells_a_script_what_kind_of_failure_it_was() {
        assert_eq!(AppError::usage("bad flag").exit_code(), 2);
        assert_eq!(AppError::Api(Api::NotSignedIn).exit_code(), 3);
        assert_eq!(
            AppError::Api(Api::NoSuchProduct("6867065002".into())).exit_code(),
            5
        );
        assert_eq!(AppError::Api(Api::Shape("odd".into())).exit_code(), 1);
    }

    #[test]
    fn every_way_of_being_signed_out_exits_the_same() {
        // Three failures, one fix from a script's point of view: get
        // credentials.
        for e in [
            Api::NotSignedIn,
            Api::SessionExpired,
            Api::LoginRefused {
                step: "password",
                detail: String::new(),
            },
        ] {
            assert_eq!(AppError::Api(e).exit_code(), 3);
        }
    }

    #[test]
    fn bot_protection_has_its_own_code_and_it_is_not_the_rate_limit_one() {
        // A script that read this as a rate limit would sleep and retry, which
        // is both useless and the thing that caused it.
        assert_eq!(AppError::Api(Api::Denied { warmed: true }).exit_code(), 8);
        assert_eq!(AppError::Api(Api::Challenged).exit_code(), 8);
    }

    #[test]
    fn being_refused_by_the_bot_manager_is_not_an_auth_exit_code() {
        // It looks like a wall, but no credential opens it; exiting 3 would
        // send a script into a sign-in loop.
        assert_ne!(AppError::Api(Api::Challenged).exit_code(), 3);
    }

    #[test]
    fn a_reported_failure_carries_its_code_without_printing_twice() {
        let e = AppError::Reported(1);
        assert!(e.silent());
        assert_eq!(e.exit_code(), 1);
    }
}
