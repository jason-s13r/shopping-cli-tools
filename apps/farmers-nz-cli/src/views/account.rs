//! Whether this tool holds a usable session.
//!
//! Unlike the other tools here there is no token to read an expiry off: the
//! Farmers session is a cookie and nothing in it says when it lapses. So this
//! reports what is actually knowable -- whether a signed-in cookie is held, and
//! whether the storefront still agrees -- rather than counting down to a time
//! it would have to invent.

use std::io::{self, Write};

use cli_kit::{Out, View};
use serde::Serialize;

#[derive(Serialize)]
pub struct AuthStatus {
    /// What the stored cookies claim.
    pub signed_in: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// What the storefront said when asked, or `None` when it was not asked --
    /// `auth status` does not spend a request unless there is a session to
    /// check.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confirmed: Option<Result<bool, String>>,
    /// Whether the Akamai cookies carried over from a previous run. Not a
    /// credential and not a fault either way; it is worth one fewer request.
    pub warmed: bool,
    /// Whether a copy of the password is in *this program's* credential store.
    /// A configured `auth.password_command` signs in just as well and is not
    /// one, so this is a claim about where the password lives.
    pub password_stored: bool,
    /// Whether `auth refresh` has something to sign in with: an account to
    /// sign in as, and either source of a password. The one worth warning
    /// about, because it decides whether a lapsed session needs a person.
    pub unattended: bool,
    pub backend: String,
}

impl View for AuthStatus {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        if !self.signed_in {
            writeln!(out, "{}. Run `farmers auth login`.", out.dim("Signed out"))?;
        } else {
            let who = self
                .name
                .as_deref()
                .or(self.email.as_deref())
                .unwrap_or("signed in");
            match &self.confirmed {
                Some(Ok(true)) => writeln!(out, "{who}, confirmed by the storefront.")?,
                // The cookie outlived the session it names. Worth saying
                // plainly: everything account-shaped will fail until a new
                // sign-in, and nothing else would explain why.
                Some(Ok(false)) => writeln!(
                    out,
                    "{who}, but {}. Run `farmers auth login`.",
                    out.warn("the storefront no longer knows this session")
                )?,
                Some(Err(e)) => writeln!(out, "{who}, {} ({e})", out.bad("unverified"))?,
                None => writeln!(out, "{who}.")?,
            }
        }
        if self.signed_in && !self.unattended {
            writeln!(
                out,
                "{}",
                out.warn(
                    "There is no password on hand, so a lapsed session has to be signed in \
                     by hand. `farmers config set auth.password_command` is the other way."
                )
            )?;
        }
        if self.warmed {
            writeln!(
                out,
                "{}",
                out.dim("Carrying a warmed session, so the next command saves a request.")
            )?;
        }
        writeln!(out, "{}", out.dim(&format!("Stored in {}.", self.backend)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cli_kit::{emit, Format};

    fn render(status: &AuthStatus) -> String {
        let mut out = Out::buffer(Format::Text);
        emit(&mut out, status).expect("writes");
        out.into_string()
    }

    fn signed_in() -> AuthStatus {
        AuthStatus {
            signed_in: true,
            email: Some("shopper@example.invalid".into()),
            name: Some("Ada Lovelace".into()),
            confirmed: Some(Ok(true)),
            warmed: false,
            password_stored: true,
            unattended: true,
            backend: "the system credential store".into(),
        }
    }

    #[test]
    fn signed_out_says_what_to_run() {
        let text = render(&AuthStatus {
            signed_in: false,
            email: None,
            name: None,
            confirmed: None,
            warmed: false,
            password_stored: false,
            unattended: false,
            backend: "a file".into(),
        });
        assert!(
            text.starts_with("Signed out. Run `farmers auth login`."),
            "{text}"
        );
    }

    #[test]
    fn a_cookie_the_storefront_has_forgotten_is_called_out_rather_than_reported_as_signed_in() {
        // This is the state that otherwise makes every account command fail
        // for no visible reason.
        let text = render(&AuthStatus {
            confirmed: Some(Ok(false)),
            ..signed_in()
        });
        assert!(text.contains("no longer knows this session"), "{text}");
        assert!(text.contains("farmers auth login"), "{text}");
    }

    #[test]
    fn signed_in_leads_with_who_rather_than_with_the_address() {
        let text = render(&signed_in());
        assert!(text.starts_with("Ada Lovelace, confirmed"), "{text}");
    }

    #[test]
    fn a_session_that_could_not_be_checked_says_so_without_claiming_a_failure() {
        let text = render(&AuthStatus {
            confirmed: Some(Err("the network is down".into())),
            ..signed_in()
        });
        assert!(text.contains("unverified"), "{text}");
        assert!(!text.contains("auth login"), "{text}");
    }

    #[test]
    fn a_session_that_cannot_renew_itself_is_warned_about_because_it_needs_a_person() {
        // The whole point of storing a password is unattended use; a session
        // that will silently stop working on a timer is worth saying out loud.
        let text = render(&AuthStatus {
            password_stored: false,
            unattended: false,
            ..signed_in()
        });
        assert!(text.contains("signed in by hand"), "{text}");
        assert!(text.contains("auth.password_command"), "{text}");
    }

    #[test]
    fn a_signed_out_status_does_not_nag_about_a_password_it_has_no_use_for() {
        // There is nothing to keep signed in, so the warning would be noise.
        let text = render(&AuthStatus {
            signed_in: false,
            email: None,
            name: None,
            confirmed: None,
            warmed: false,
            password_stored: false,
            unattended: false,
            backend: "a file".into(),
        });
        assert!(!text.contains("by hand"), "{text}");
    }

    #[test]
    fn a_carried_warm_up_is_mentioned_because_it_is_a_request_not_spent() {
        let text = render(&AuthStatus {
            warmed: true,
            ..signed_in()
        });
        assert!(text.contains("saves a request"), "{text}");
    }
}
