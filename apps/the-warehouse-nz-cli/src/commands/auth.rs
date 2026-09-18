//! `auth` -- signing in, staying signed in, and signing out.
//!
//! One credential, unlike the Kmart tool next door: the storefront authorises
//! by cookie, and the cookies come from an ordinary form POST that can simply
//! be run again. So there is nothing here to renew *from* -- `refresh` is
//! `login` with the typing already answered, and the only question worth asking
//! first is whether it needs to run at all.

use cli_kit::{emit, human_duration, prompt, prompt_password, Out, View};
use serde::Serialize;
use std::io::Write;
use std::time::Duration;

use crate::app::App;
use crate::cli::AuthAction;
use crate::error::{AppError, AppResult};

pub async fn run(app: &App, action: AuthAction) -> AppResult<()> {
    match action {
        AuthAction::Login {
            email,
            password_command,
            no_store_password,
        } => login(app, email, password_command, no_store_password).await,
        AuthAction::Refresh { force } => refresh(app, force).await,
        AuthAction::Status => status(app),
        AuthAction::Logout => logout(app),
    }
}

async fn login(
    app: &App,
    email: Option<String>,
    password_command: Option<String>,
    no_store_password: bool,
) -> AppResult<()> {
    let secrets = app.secrets();
    let email = match email {
        Some(email) => email,
        None => prompt("Email")?,
    };

    // A command beats a prompt, so a password manager never has to be typed out
    // of.
    let command = password_command.or_else(|| app.config.auth.password_command.clone());
    let password = match &command {
        Some(command) => net_kit::run::capturing("password_command", command).await?,
        None => prompt_password("Password")?,
    };

    let session = sign_in(app, &email, &password).await?;
    let stored = twlnz_api::StoredSession::of(&session, Some(email));
    stored.save(&secrets)?;

    // Kept only when it can be used: `auth refresh` reads the password back,
    // and without one an expired session stops every account command until
    // someone signs in by hand.
    if !no_store_password && app.config.auth.store_password && command.is_none() {
        net_kit::password::save(&secrets, &password)?;
    }

    report(app, &mut app.out(), Some(&stored), &secrets)
}

fn status(app: &App) -> AppResult<()> {
    let secrets = app.secrets();
    let stored = twlnz_api::StoredSession::load(&secrets)?;
    emit(&mut app.out(), &status_of(app, stored.as_ref(), &secrets))?;
    Ok(())
}

/// Sign in again without anyone typing.
///
/// The scheduled counterpart to `login`, and the reason the password is kept at
/// all. What it does not do is renew anything: there is no grant to spend here,
/// so a session that needs replacing is replaced by running the login form
/// again, and the work is in deciding whether it does.
async fn refresh(app: &App, force: bool) -> AppResult<()> {
    let secrets = app.secrets();
    let stored = twlnz_api::StoredSession::load(&secrets)?;
    let session = stored
        .as_ref()
        .map(twlnz_api::StoredSession::session)
        .unwrap_or_default();

    // Resolved before anything is spent: it decides whether a lapsed session is
    // recoverable at all, and a config file is cheaper to read than the
    // storefront is to ask. A configured command beats the stored copy.
    let source =
        net_kit::password::Source::resolve(app.config.auth.password_command.as_deref(), &secrets)?;
    let email = stored.as_ref().and_then(|s| s.email.clone());
    let reauthable = email.is_some() && source.is_some();

    // Nothing signed in and nothing to sign in with. An error rather than a
    // remark, because nobody is reading: a scheduled run that cannot tell
    // "signed in again" from "did nothing" is the failure being avoided.
    if !session.account() && !reauthable {
        return Err(twlnz_api::Error::NotSignedIn.into());
    }

    let mut out = app.out();

    // Cheapest first, and the cheap half is free: the shopper token is a
    // readable JWT, so a lapsed one is known without asking anyone. A token
    // that has not lapsed still costs one request, because the storefront can
    // have dropped the session at its end and the token would not know.
    if !force && session.account() && !session.lapsed() && verify(app).await? {
        note(&mut out, "Nothing needed signing in again.")?;
        return report(app, &mut out, stored.as_ref(), &secrets);
    }

    // Said out loud in a moment, so a log carries the reason and not just the
    // action.
    let why = if force {
        "--force was given"
    } else if !session.account() {
        "there is no session"
    } else if session.lapsed() {
        "the session has expired"
    } else {
        "the storefront no longer accepts the session"
    };

    // Said in full, and given the auth exit code rather than a usage one: this
    // is the state a wrapper has to be able to tell apart, because it is the
    // only one that needs a person.
    let Some(email) = email else {
        eprintln!("twlnz: {why}, and nothing on file names an account to sign in as");
        eprintln!("twlnz: run `twlnz auth login`");
        return Err(AppError::Reported(3));
    };
    let password = match source {
        Some(source) => source.password().await?,
        None => None,
    };
    let Some(password) = password else {
        eprintln!("twlnz: {why}, and there is no password to sign in again with");
        eprintln!("twlnz: run `twlnz auth login`, or set `auth.password_command`");
        return Err(AppError::Reported(3));
    };

    note(&mut out, &format!("{why}; signing in again."))?;
    let session = sign_in(app, &email, &password).await?;
    let stored = twlnz_api::StoredSession::of(&session, Some(email));
    stored.save(&secrets)?;

    report(app, &mut out, Some(&stored), &secrets)
}

/// Whether the storefront still recognises the stored session, by spending one
/// request.
///
/// A refusal is an answer rather than a failure -- it is what sends `refresh`
/// to the login form -- but anything else is propagated. Signing in again is
/// the wrong response to a connection that is down, and the wrong story to tell
/// about a Cloudflare challenge, which would refuse the login form in exactly
/// the same way.
///
/// The reauth is taken off the client on purpose. `Client::verify` does not
/// sign itself in, but a client that could would make this always answer yes,
/// and spend a password to do it.
async fn verify(app: &App) -> AppResult<bool> {
    let probe = app.client()?.with_reauth(None);
    match probe.verify().await {
        Ok(good) => Ok(good),
        Err(e) if e.is_lapsed() => Ok(false),
        Err(e) => Err(e.into()),
    }
}

/// An aside for a person, skipped when the output is JSON.
fn note(out: &mut Out, line: &str) -> std::io::Result<()> {
    if out.is_json() {
        return Ok(());
    }
    let dimmed = out.dim(line);
    writeln!(out, "{dimmed}")
}

fn report(
    app: &App,
    out: &mut Out,
    stored: Option<&twlnz_api::StoredSession>,
    secrets: &net_kit::Secrets,
) -> AppResult<()> {
    emit(out, &status_of(app, stored, secrets))?;
    Ok(())
}

/// What `auth status` says, from a session already in hand.
fn status_of(
    app: &App,
    stored: Option<&twlnz_api::StoredSession>,
    secrets: &net_kit::Secrets,
) -> Status {
    let session = stored.map(twlnz_api::StoredSession::session);
    let stored_password = password_stored(secrets);
    Status {
        signed_in: session.as_ref().is_some_and(twlnz_api::Session::account),
        account: stored.and_then(|s| s.email.clone()),
        expires_in: session.as_ref().and_then(|s| {
            s.expires_at()
                .map(|exp| exp.saturating_sub(net_kit::jwt::now_secs()))
        }),
        password_stored: stored_password,
        unattended: stored_password || app.config.auth.password_command.is_some(),
    }
}

/// Whether there is a password *in the credential store*, which is what
/// `password_stored` says. A configured `auth.password_command` signs in just
/// as well but is not a copy this program holds, and reporting it as one would
/// be a claim about where the password lives.
fn password_stored(secrets: &net_kit::Secrets) -> bool {
    net_kit::password::load(secrets).unwrap_or(None).is_some()
}

/// Run the login form, with a clean session.
///
/// Clean on purpose: reusing whatever is stored would send an expired account
/// cookie along with the form and leave the failure looking like a bad
/// password.
async fn sign_in(app: &App, email: &str, password: &str) -> AppResult<twlnz_api::Session> {
    let http = app.http()?;
    let trace: twlnz_api::auth::Trace<'_> = &|m: &str| {
        if app.env.debug {
            eprintln!("twlnz: {m}");
        }
    };
    Ok(twlnz_api::auth::login(&http, &app.endpoints(), email, password, trace).await?)
}

fn logout(app: &App) -> AppResult<()> {
    let secrets = app.secrets();
    // Both, always. Leaving the password behind after a logout is the kind of
    // surprise that only shows up much later.
    let had_session = twlnz_api::StoredSession::clear(&secrets)?;
    let had_password = net_kit::password::clear(&secrets).unwrap_or(false);
    emit(
        &mut app.out(),
        &LoggedOut {
            session: had_session,
            password: had_password,
        },
    )
    .map_err(AppError::from)
}

#[derive(Serialize)]
struct Status {
    signed_in: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    account: Option<String>,
    /// Seconds. The account token is a readable JWT, unlike the Woolworths one,
    /// so this is a fact rather than an estimate.
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_in: Option<u64>,
    /// Whether a copy of the password is in *this program's* credential store.
    /// A configured `auth.password_command` signs in just as well and is not
    /// one, so this is a claim about where the password lives.
    password_stored: bool,
    /// Whether `auth refresh` has something to sign in with: either of the two
    /// above. The one worth warning about, because it is what decides whether
    /// an expired session needs a person.
    unattended: bool,
}

impl View for Status {
    fn text(&self, out: &mut Out) -> std::io::Result<()> {
        if !self.signed_in {
            return writeln!(out, "{}. Run `twlnz auth login`.", out.dim("Signed out"));
        }
        let who = self.account.as_deref().unwrap_or("signed in");
        match self.expires_in {
            Some(secs) => writeln!(
                out,
                "{who}, for another {}.",
                human_duration(Duration::from_secs(secs))
            )?,
            None => writeln!(out, "{who}.")?,
        }
        if !self.unattended {
            writeln!(
                out,
                "{}",
                out.dim(
                    "There is no password on hand, so an expired session has to be \
                     signed in by hand."
                )
            )?;
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct LoggedOut {
    session: bool,
    password: bool,
}

impl View for LoggedOut {
    fn text(&self, out: &mut Out) -> std::io::Result<()> {
        match (self.session, self.password) {
            (false, false) => writeln!(out, "Nothing to forget."),
            (_, true) => writeln!(out, "Signed out, and the stored password is gone."),
            (true, false) => writeln!(out, "Signed out."),
        }
    }
}
