//! `auth` -- signing in, staying signed in, and signing out.
//!
//! One credential, and no grant behind it. The storefront authorises by cookie
//! and the cookie comes from an ordinary form POST, so there is nothing here to
//! *renew from*: `refresh` is `login` with the typing already answered, and the
//! work is in deciding whether it needs to run at all.
//!
//! That decision matters more here than at the other retailers, because of the
//! third answer this storefront can give. A session can be good, or lapsed, or
//! **the bot manager can refuse to say** -- and signing in again would be
//! refused in exactly the same way. So a refusal stops `refresh` rather than
//! sending it to the login form, which would spend a password to learn nothing
//! and report a credential problem that does not exist.

use std::io::Write;

use cli_kit::{emit, prompt, prompt_or_stdin, prompt_password, Out, View};
use farmers_api::StoredSession;
use serde::Serialize;

use crate::app::App;
use crate::cli::AuthAction;
use crate::error::{AppError, AppResult};

pub async fn run(app: &App, action: AuthAction) -> AppResult<()> {
    match action {
        AuthAction::Login {
            email,
            stdin,
            password_command,
            no_store_password,
        } => login(app, email, stdin, password_command, no_store_password).await,
        AuthAction::Refresh { force } => refresh(app, force).await,
        AuthAction::Status => status(app).await,
        AuthAction::Logout => logout(app).await,
    }
}

async fn login(
    app: &App,
    email: Option<String>,
    stdin: bool,
    password_command: Option<String>,
    no_store_password: bool,
) -> AppResult<()> {
    let secrets = app.secrets();
    let email = match email {
        Some(email) => email,
        None => prompt("Email").map_err(|e| AppError::usage(e.to_string()))?,
    };

    // Asked before a password is, because being signed in already is not
    // worth typing for. Costs nothing unless the stored session claims this
    // account, and the storefront is what settles it -- a stored cookie looks
    // identical from here whether or not the other end still honours it.
    let client = app.client()?;
    if let Some(account) = client.signed_in_as(email.trim()).await? {
        return emit(
            &mut app.out(),
            &AlreadySignedIn {
                account: account
                    .name()
                    .or(account.email)
                    .unwrap_or_else(|| email.trim().to_string()),
            },
        )
        .map_err(AppError::from);
    }

    // A command beats a prompt, so a password manager never has to be typed
    // out of.
    let command = password_command.or_else(|| app.config.auth.password_command.clone());
    let password = match (&command, stdin) {
        (Some(command), _) => {
            net_kit::password::Source::Command(command.clone())
                .password()
                .await?
        }
        (None, true) => prompt_or_stdin("Password").map_err(|e| AppError::usage(e.to_string()))?,
        (None, false) => prompt_password("Password").map_err(|e| AppError::usage(e.to_string()))?,
    };
    if email.trim().is_empty() || password.is_empty() {
        return Err(AppError::usage("an email and a password are both needed"));
    }

    // The client files the session itself, so there is one path to the
    // credential store rather than two that could drift.
    let account = client.login(email.trim(), &password).await?;

    // Kept only when it can be used, and only when this program would be the
    // one holding it: a configured command is the account's real source of
    // truth and a copy beside it is a second place to leak from.
    if !no_store_password && app.config.auth.store_password && command.is_none() {
        net_kit::password::save(&secrets, &password)?;
    }

    // Its own view rather than the status one: after a login the interesting
    // fact is not "you are signed in" -- that was the command -- but whether
    // this will still be true tomorrow without anyone here.
    emit(
        &mut app.out(),
        &SignedIn {
            account: account
                .name()
                .or(account.email.clone())
                .unwrap_or_else(|| email.trim().to_string()),
            password_kept: net_kit::password::load(&secrets).unwrap_or(None).is_some(),
            password_command: command.is_some(),
            backend: secrets.backend().describe().to_string(),
        },
    )?;
    Ok(())
}

/// Sign in again without anyone typing.
///
/// The scheduled counterpart to `login`, and the reason a password is kept at
/// all.
async fn refresh(app: &App, force: bool) -> AppResult<()> {
    let secrets = app.secrets();
    let stored = StoredSession::load(&secrets)?;
    let session = stored
        .as_ref()
        .map(StoredSession::session)
        .unwrap_or_default();

    // Resolved before anything is spent: it decides whether a lapsed session
    // is recoverable at all, and a config file is cheaper to read than the
    // storefront is to ask.
    let email = session.email.clone();
    let source =
        net_kit::password::Source::resolve(app.config.auth.password_command.as_deref(), &secrets)?;
    let reauthable = email.is_some() && source.is_some();

    // Nothing signed in and nothing to sign in with. An error rather than a
    // remark, because nobody is reading: a scheduled run that cannot tell
    // "signed in again" from "did nothing" is the failure being avoided.
    if !session.account() && !reauthable {
        return Err(farmers_api::Error::NotSignedIn.into());
    }

    let mut out = app.out();
    let client = app.client()?;

    // Cheapest first. Unlike the other tools here there is no readable expiry
    // to check for free -- the session is a cookie and says nothing about
    // itself -- so this costs one request whenever there is a session at all.
    //
    // A refusal is propagated rather than treated as a lapse. See the module
    // docs: the login form would be refused identically, so signing in again
    // is both futile and the wrong story to tell.
    if !force && session.account() {
        match client.verify().await {
            Ok(true) => {
                note(&mut out, "Nothing needed signing in again.")?;
                emit(&mut out, &status_of(app, true, None, &secrets))?;
                return Ok(());
            }
            Ok(false) => {}
            Err(e) if e.is_denied() => return Err(e.into()),
            Err(e) => return Err(e.into()),
        }
    }

    // Said out loud in a moment, so a log carries the reason and not just the
    // action.
    let why = if force {
        "--force was given"
    } else if !session.account() {
        "there is no session"
    } else {
        "the storefront no longer accepts the session"
    };

    // Said in full, and given the auth exit code rather than a usage one: this
    // is the state a wrapper has to tell apart, because it is the only one
    // that needs a person.
    let Some(_) = email else {
        eprintln!("farmers: {why}, and nothing on file names an account to sign in as");
        eprintln!("farmers: run `farmers auth login`");
        return Err(AppError::Reported(3));
    };
    if source.is_none() {
        eprintln!("farmers: {why}, and there is no password to sign in again with");
        eprintln!("farmers: run `farmers auth login`, or set `auth.password_command`");
        return Err(AppError::Reported(3));
    }

    note(&mut out, &format!("{why}; signing in again."))?;
    let account = client.renew().await?;
    emit(
        &mut out,
        &status_of(app, account.signed_in, account.name(), &secrets),
    )?;
    Ok(())
}

async fn status(app: &App) -> AppResult<()> {
    let secrets = app.secrets();
    let session = StoredSession::load(&secrets)?
        .map(|s| s.session())
        .unwrap_or_default();

    // Asked only when there is something to ask about. A signed-out `auth
    // status` spending a request against a host that counts them would be a
    // poor trade for news it already has.
    let mut view = status_of(app, session.account(), None, &secrets);
    view.warmed = session.warmed();
    if session.account() {
        view.confirmed = Some(match app.client()?.verify().await {
            Ok(good) => Ok(good),
            Err(e) => Err(e.to_string()),
        });
    }
    emit(&mut app.out(), &view)?;
    Ok(())
}

async fn logout(app: &App) -> AppResult<()> {
    let secrets = app.secrets();
    let client = app.client()?;
    let was_signed_in = client.is_signed_in();
    if was_signed_in {
        client.sign_out().await?;
    }
    // Both, always, and whether or not there was a session: leaving the
    // password behind after a logout is the kind of surprise that only shows
    // up much later.
    let had_password = net_kit::password::clear(&secrets).unwrap_or(false);
    emit(
        &mut app.out(),
        &LoggedOut {
            session: was_signed_in,
            password: had_password,
        },
    )?;
    Ok(())
}

/// What `auth status` says.
fn status_of(
    app: &App,
    signed_in: bool,
    name: Option<String>,
    secrets: &net_kit::Secrets,
) -> crate::views::AuthStatus {
    let session = StoredSession::load(secrets)
        .ok()
        .flatten()
        .map(|s| s.session())
        .unwrap_or_default();
    let stored_password = net_kit::password::load(secrets).unwrap_or(None).is_some();
    crate::views::AuthStatus {
        signed_in,
        email: session.email.clone(),
        name,
        confirmed: None,
        warmed: session.warmed(),
        password_stored: stored_password,
        // Either source will do, but an account to sign in *as* is needed too:
        // a password with no email attached signs in as nobody.
        unattended: session.email.is_some()
            && (stored_password || app.config.auth.password_command.is_some()),
        backend: secrets.backend().describe().to_string(),
    }
}

/// An aside for a person, skipped when the output is JSON.
fn note(out: &mut Out, line: &str) -> std::io::Result<()> {
    if out.is_json() {
        return Ok(());
    }
    writeln!(out, "{line}")
}

#[derive(Serialize)]
struct SignedIn {
    account: String,
    /// Whether a copy of the password went into the credential store.
    password_kept: bool,
    /// Whether a configured command will supply it instead, which is the
    /// better of the two and means nothing was written.
    password_command: bool,
    backend: String,
}

impl View for SignedIn {
    fn text(&self, out: &mut Out) -> std::io::Result<()> {
        writeln!(out, "Signed in as {}.", self.account)?;
        // Said every time, because it is a fact about where a password now
        // lives and nobody should have to go looking for it.
        let unattended = match (self.password_command, self.password_kept) {
            (true, _) => out.dim(
                "The configured password command will sign in again, so no password was kept.",
            ),
            (false, true) => {
                out.dim("Password kept, so `farmers auth refresh` can sign in again unattended.")
            }
            (false, false) => {
                out.warn("No password kept, so a lapsed session will need signing in by hand.")
            }
        };
        writeln!(out, "{unattended}")?;
        writeln!(out, "{}", out.dim(&format!("Stored in {}.", self.backend)))
    }
}

/// Nothing happened, and saying so is the whole point.
#[derive(Serialize)]
struct AlreadySignedIn {
    account: String,
}

impl View for AlreadySignedIn {
    fn text(&self, out: &mut Out) -> std::io::Result<()> {
        writeln!(out, "Already signed in as {}.", self.account)?;
        // Named because this is the answer to "then how do I sign in as
        // somebody else", which is the only reason to be here having read the
        // line above.
        writeln!(
            out,
            "{}",
            out.dim("Nothing to do. `farmers auth logout` first to sign in as someone else.")
        )
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
