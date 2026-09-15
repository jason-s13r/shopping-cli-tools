//! Whether this tool holds usable credentials.

use std::io::{self, Write};
use std::time::Duration;

use cli_kit::{human_duration, Out, View};
use serde::Serialize;

#[derive(Serialize)]
pub struct AuthStatus {
    pub signed_in: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Seconds left on the access token. The bearer is a readable JWT, so this
    /// is a fact rather than an estimate -- absent only when there is no token
    /// to read, which a refresh fixes silently.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expires_in: Option<u64>,
    /// Whether a lapsed token can be renewed without a password.
    pub can_refresh: bool,
    pub backend: String,
}

impl View for AuthStatus {
    fn text(&self, out: &mut Out) -> io::Result<()> {
        if !self.signed_in {
            return writeln!(out, "{}. Run `mitre10 auth login`.", out.dim("Signed out"));
        }
        let who = self.email.as_deref().unwrap_or("signed in");
        match self.expires_in {
            Some(secs) => writeln!(
                out,
                "{who}, for another {}.",
                human_duration(Duration::from_secs(secs))
            )?,
            // Ordinary and silent: a stale token is minted again by the next
            // command. Having nothing to mint it *from* is the line below.
            None => writeln!(out, "{who}, token will be minted.")?,
        }
        writeln!(
            out,
            "{}",
            match self.can_refresh {
                true => out.dim("Renewable without a password."),
                false => out.warn("No refresh token, so the next lapse needs a password."),
            }
        )?;
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
            email: Some("someone@example.test".into()),
            expires_in: Some(840),
            can_refresh: true,
            backend: "the system credential store".into(),
        }
    }

    #[test]
    fn signed_out_says_what_to_run_and_nothing_else() {
        let text = render(&AuthStatus {
            signed_in: false,
            email: None,
            expires_in: None,
            can_refresh: false,
            backend: "a file".into(),
        });
        assert_eq!(text, "Signed out. Run `mitre10 auth login`.\n");
    }

    #[test]
    fn signed_in_leads_with_the_account_and_how_long_it_has() {
        let text = render(&signed_in());
        assert!(
            text.starts_with("someone@example.test, for another 14m."),
            "{text}"
        );
    }

    #[test]
    fn a_session_with_nothing_to_renew_from_warns_because_that_one_needs_a_person() {
        let text = render(&AuthStatus {
            can_refresh: false,
            ..signed_in()
        });
        assert!(text.contains("needs a password"), "{text}");
    }
}
