//! `auth` -- getting the two credentials, and giving them up.
//!
//! Two independent things, and this command handles both because they fail in
//! ways a person would confuse: a **bearer token** says who you are, and
//! **Akamai cookies** say you are a browser. Having one without the other is
//! normal, and `status` reports them apart.
//!
//! Both come from a browser, for different reasons. The cookies because
//! passing the bot check means running its script. The token because the one
//! step of Auth0's login that would mint one -- the password submit -- is
//! behind that same check; see `kmart_api::auth`. The token half is the
//! better bargain, though: it is a *refresh* token, and the endpoint that
//! spends it is not guarded, so it is copied once and renews itself
//! thereafter. The cookies last about a day.

use std::io::{Read, Write};
use std::time::Duration;

use cli_kit::{emit, human_duration, prompt, prompt_password, Out, View};
use kmart_api::{Country, Session, StoredSession};
use serde::Serialize;

use crate::app::App;
use crate::cli::AuthAction;
use crate::error::{AppError, AppResult};

pub async fn run(app: &App, action: AuthAction) -> AppResult<()> {
    match action {
        AuthAction::Login {
            email,
            password_command,
            no_store_password,
            headful,
            direct,
        } => {
            login(
                app,
                email,
                password_command,
                no_store_password,
                !headful,
                direct,
            )
            .await
        }
        AuthAction::Import { file } => import(app, &file),
        AuthAction::Token { token } => token_import(app, token).await,
        AuthAction::Refresh {
            headful,
            direct,
            force,
        } => refresh(app, !headful, direct, force).await,
        AuthAction::Status => status(app),
        AuthAction::Logout => logout(app),
    }
}

async fn login(
    app: &App,
    email: Option<String>,
    password_command: Option<String>,
    no_store_password: bool,
    headless: bool,
    direct: bool,
) -> AppResult<()> {
    let secrets = app.secrets();
    let email = match email {
        Some(email) => email,
        None => prompt("Email")?,
    };

    // A command beats a prompt, so a password manager never has to be typed
    // out of -- `--password-command 'op read "op://Vault/Kmart/password"'`
    // and the password never touches this process's output or its arguments.
    let command = password_command.or_else(|| app.config.auth.password_command.clone());
    let password = match &command {
        Some(command) => net_kit::run::capturing("password_command", command).await?,
        None => prompt_password("Password")?,
    };

    // The no-browser experiment. Its own path because it earns no cookies and
    // stores only the token, and because what it is for is being watched fail:
    // it narrates each step and stops at whatever answers the password submit.
    if direct {
        return direct_login(app, &email, &password).await;
    }

    let mut out = app.out();
    if !out.is_json() {
        let how = match headless {
            true => "Signing in through a browser (headless). Pass --headful to watch it.",
            false => "Signing in through a browser.",
        };
        writeln!(out, "{}", out.dim(how))?;
    }

    let signed_in = crate::browser::login(
        app.env.browser_python.as_deref(),
        &app.paths.state_dir,
        app.country.origin(),
        &email,
        &password,
        headless,
        app.env.debug,
    )
    .await?;

    // One run earns both credentials, so both are kept. The cookies are
    // merged rather than replacing, so a country this run did not visit keeps
    // whatever it had.
    let mut stored = StoredSession::load(&secrets)?.unwrap_or_default();
    stored.tokens = Some(kmart_api::Tokens::from_refresh(&signed_in.refresh_token));
    stored.email = signed_in.email.clone().or(Some(email.clone()));
    // Which storefront minted it. The two countries are separate Auth0
    // applications, so this is what a later renewal needs to pick the right
    // one -- a session signed in on kmart.com.au cannot be renewed as the New
    // Zealand app, whatever country the next command asks about.
    stored.auth_country = Some(app.country.code().to_string());
    for (code, cookies) in &signed_in.cookies {
        if !cookies.is_empty() {
            stored.cookies.insert(code.clone(), cookies.clone());
        }
    }
    stored.save(&secrets)?;

    // Kept only when it can be used. Less load-bearing here than for the other
    // tools in this repo -- the refresh token renews without it -- so this is
    // for the day that token is refused.
    let keep = !no_store_password && app.config.auth.store_password && command.is_none();
    if keep {
        net_kit::password::save(&secrets, &password)?;
    }

    remember_country(app, &mut out)?;

    let session = stored.session();
    emit(
        &mut out,
        &status_of(app, &session, stored.email.clone(), keep),
    )?;
    Ok(())
}

/// Keep asking the storefront that was just signed in to.
///
/// The second setting this program writes without being asked, after the
/// visitor id, and for the same reason: the alternative is worse. A session is
/// bound to the storefront that minted it -- one Auth0 application per country
/// -- so signing in to kmart.com.au and then having every command quote New
/// Zealand prices off a token that country's gateway will not take is not a
/// state worth being able to reach by doing nothing.
///
/// This is why `--country` on a login is not the one-command flag it is
/// everywhere else. Announced when it changes which shop answers, silent when
/// it only writes down what was already happening.
///
/// Best effort: a config directory that cannot be written should not turn a
/// successful sign-in into a failed command.
fn remember_country(app: &App, out: &mut Out) -> std::io::Result<()> {
    if app.config.country == Some(app.country) {
        return Ok(());
    }
    let before = app.config.country.unwrap_or(crate::app::DEFAULT_COUNTRY);
    let mut config = app.config.clone();
    config.country = Some(app.country);
    if app.save(&config).is_err() || before == app.country {
        return Ok(());
    }
    note(
        out,
        &format!(
            "Now using {} ({}), which is where this signed in.",
            app.country.name(),
            app.country
        ),
    )
}

/// Sign in with no browser, by replaying Auth0's login as direct requests.
///
/// The point of `--direct`: the flow the browser walks, made by this program's
/// own emulation client instead. Against the live site Akamai answers the
/// password submit and this ends in [`kmart_api::Error::Challenged`] -- kept so
/// that can be measured rather than assumed, and so a policy change that opened
/// the step would be noticed. Each step is printed to stderr, so where it stops
/// is visible without `KMART_DEBUG`.
///
/// It stores only the token, and only if one is minted. No cookies are earned
/// this way -- validating Akamai's admission needs the browser's sensor script
/// to run -- so a token from here still wants `auth import` for the gateway.
async fn direct_login(app: &App, email: &str, password: &str) -> AppResult<()> {
    let secrets = app.secrets();
    let http = net_kit::http::build(kmart_api::client_spec())
        .map_err(|e| AppError::usage(format!("building the HTTP client: {e}")))?;
    // Current identifiers, so the flow is built against what the storefront
    // serves today rather than what shipped -- the same set the browser path
    // and every other command resolve.
    let endpoints = app.live_endpoints(&http, None).await;

    // Any admission this session already holds, from a browser export
    // (`auth import`) or an earlier browser login. The auth host is under
    // `.kmart.com.au`, so the Australian bucket is the one that covers it --
    // and feeding it in is the whole experiment: whether a `_abck` another
    // client validated lets this one past Akamai's guard on the password step.
    let mut stored = StoredSession::load(&secrets)?.unwrap_or_default();
    let admission = stored.session().admission(Country::Au);

    // Always narrated: watching where it stops is the whole reason this exists.
    let trace: kmart_api::auth::Trace<'_> =
        &|step: &str, detail: &str| eprintln!("kmart: [{step}] {detail}");

    let tokens = kmart_api::auth::login(&endpoints, email, password, &admission, trace).await?;

    stored.tokens = Some(tokens);
    stored.email = Some(email.to_string());
    stored.auth_country = Some(app.country.code().to_string());
    stored.save(&secrets)?;

    let mut out = app.out();
    if !out.is_json() {
        writeln!(
            out,
            "{}",
            out.dim("Signed in directly, no browser. No bot-check cookies were earned this way.")
        )?;
    }
    let session = stored.session();
    emit(
        &mut out,
        &status_of(app, &session, stored.email.clone(), false),
    )?;
    Ok(())
}

/// Renew whatever has lapsed, with nobody at the keyboard.
///
/// The two credentials run on different clocks and this renews both, cheapest
/// first. The refresh token costs one request to an endpoint Akamai does not
/// guard; the cookies cost a browser, because a browser is the only thing that
/// earns them. So a run that only needed the token never opens one.
///
/// **The cookies are tested by spending a request, not by looking at them.**
/// There is no clock in them -- `_abck` is present or it is not, and one that
/// expired an hour ago is indistinguishable from a good one until the gateway
/// answers. Reporting a renewal on the strength of a stale cookie is the exact
/// failure this command exists to prevent, since nobody is reading the output.
async fn refresh(app: &App, headless: bool, direct: bool, force: bool) -> AppResult<()> {
    let secrets = app.secrets();
    let mut stored = StoredSession::load(&secrets)?.unwrap_or_default();

    // Resolved before anything is spent: it decides whether a refused token is
    // recoverable at all, and a config file is cheaper to read than Auth0 is
    // to ask.
    let source =
        net_kit::password::Source::resolve(app.config.auth.password_command.as_deref(), &secrets)?;
    let reauthable = stored.email.is_some() && source.is_some();

    // Nothing to renew and nothing to renew it with. An error rather than a
    // remark, because nobody is reading: a scheduled run that cannot tell
    // "renewed" from "did nothing" is the failure being avoided.
    if !stored.session().signed_in() && !reauthable {
        return Err(kmart_api::Error::NotSignedIn.into());
    }

    // The storefront that signed in, which is not always the one being asked
    // about. They are separate Auth0 applications and a token minted by one is
    // not renewable under the other, so `--country nz` over a session from
    // kmart.com.au still has to renew as the Australian application.
    let minted_by = stored.auth_country();
    let http = net_kit::http::build(kmart_api::client_spec())
        .map_err(|e| AppError::usage(format!("building the HTTP client: {e}")))?;
    let endpoints = app.live_endpoints(&http, minted_by).await;

    let trace: kmart_api::auth::Trace<'_> = &|step: &str, detail: &str| {
        if app.env.debug {
            eprintln!("kmart: [{step}] {detail}");
        }
    };

    let mut out = app.out();
    let mut renewed = false;
    // Not spent while it is still good. Auth0 rotates the grant on use, so
    // renewing a token that has not lapsed buys nothing and puts one more
    // write between this session and the next command.
    let mut token_ok = stored.tokens.as_ref().is_some_and(|t| !t.lapsed());
    if !token_ok || force {
        if let Some(grant) = stored.tokens.as_ref().and_then(|t| t.refresh.clone()) {
            match kmart_api::auth::refresh(&endpoints, &grant, trace).await {
                Ok(mut fresh) => {
                    // Auth0 rotates only sometimes, and keeping the old grant
                    // when it does not is the difference between a session
                    // that renews forever and one that dies in fifteen minutes.
                    if fresh.refresh.is_none() {
                        fresh.refresh = Some(grant);
                    }
                    stored.tokens = Some(fresh);
                    stored.save(&secrets)?;
                    token_ok = true;
                    renewed = true;
                    note(&mut out, "Renewed from the refresh token.")?;
                }
                // Auth0 answered, and said no. A password is the only way past
                // that, so this falls through to one rather than failing here.
                Err(
                    e @ (kmart_api::Error::LoginRefused { .. }
                    | kmart_api::Error::Challenged { .. }),
                ) if reauthable => {
                    token_ok = false;
                    if app.env.debug {
                        eprintln!("kmart: the refresh token was refused: {e}");
                    }
                }
                // Either nobody answered, or nobody can sign in again. A
                // browser fixes neither.
                Err(e) => return Err(e.into()),
            }
        }
    }

    // Skipped when the token is already known to be bad: the gateway would
    // refuse whatever the cookies are, so the answer would say nothing about
    // them.
    if token_ok && !force && works(app).await? {
        if !renewed {
            note(&mut out, "Nothing needed renewing.")?;
        }
        return report(app, &mut out, &stored, &secrets);
    }

    let why = match token_ok {
        true => "the gateway refused the bot-check cookies",
        false => "the refresh token is gone or was refused",
    };

    // Said in full and given an auth exit code rather than a usage one: this
    // is the state a wrapper has to be able to tell apart, because it is the
    // only one that needs a person.
    let Some(email) = stored.email.clone() else {
        eprintln!("kmart: {why}, and the session names no account to sign in as");
        eprintln!("kmart: run `kmart auth login`");
        return Err(AppError::Reported(3));
    };
    let password = match source {
        Some(source) => source.password().await?,
        None => None,
    };
    let Some(password) = password else {
        eprintln!("kmart: {why}, and there is no password to sign in again with");
        eprintln!("kmart: run `kmart auth login`, or set `auth.password_command`");
        return Err(AppError::Reported(3));
    };

    if direct {
        return direct_login(app, &email, &password).await;
    }

    let how = match headless {
        true => " (headless)",
        false => "",
    };
    note(
        &mut out,
        &format!("{why}; signing in through a browser{how}."),
    )?;

    // The storefront that signed in signs in again: moving the session to
    // whatever `--country` happens to say would swap the Auth0 application out
    // from under it. Nothing is lost by staying -- the script collects the
    // other country's cookies before it finishes, so the country in use ends
    // up admitted whichever one minted the token.
    let country = minted_by.unwrap_or(app.country);
    let signed_in = crate::browser::login(
        app.env.browser_python.as_deref(),
        &app.paths.state_dir,
        country.origin(),
        &email,
        &password,
        headless,
        app.env.debug,
    )
    .await?;

    stored.tokens = Some(kmart_api::Tokens::from_refresh(&signed_in.refresh_token));
    stored.email = signed_in.email.clone().or(Some(email));
    stored.auth_country = Some(country.code().to_string());
    for (code, cookies) in &signed_in.cookies {
        if !cookies.is_empty() {
            stored.cookies.insert(code.clone(), cookies.clone());
        }
    }
    stored.save(&secrets)?;

    report(app, &mut out, &stored, &secrets)
}

/// Whether the gateway still accepts this session, by spending one request.
///
/// **The cookies are what this asks about**, and deliberately only them. The
/// token half needs no request: its expiry is read from the JWT's own `exp`,
/// which is the claim the gateway enforces, and Auth0 accepting the refresh a
/// moment ago is the rest of the answer. Admission is the half with nothing to
/// read -- `_abck` is present or it is not -- so it is the half worth a call.
///
/// The call is the one `doctor` makes for the same reason: postcode
/// suggestions need no bearer token, so the answer turns on the cookies alone
/// and cannot be muddied by the account half. An account query would be the
/// wrong instrument twice over -- it asks two questions at once, and the
/// gateway does not serve the same account schema in both countries.
///
/// A refusal is an answer rather than a failure -- it is what sends `refresh`
/// to the browser -- but anything else is propagated, because opening a
/// browser is the wrong response to a flaky connection.
async fn works(app: &App) -> AppResult<bool> {
    let client = app.client().await?;
    match client.postcodes(app.probe_postcode()).await {
        Ok(_) => Ok(true),
        Err(
            kmart_api::Error::Challenged { .. }
            | kmart_api::Error::NoSession { .. }
            | kmart_api::Error::NotSignedIn
            | kmart_api::Error::SessionExpired
            | kmart_api::Error::SessionUnrenewable
            | kmart_api::Error::LoginRefused { .. },
        ) => Ok(false),
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

/// Whether there is a password *in the credential store*, which is what
/// `password_stored` says. A configured `password_command` renews just as well
/// but is not a copy this program holds, and reporting it as one would be a
/// claim about where the password lives.
fn password_stored(secrets: &net_kit::Secrets) -> bool {
    net_kit::password::load(secrets).unwrap_or(None).is_some()
}

fn report(
    app: &App,
    out: &mut Out,
    stored: &StoredSession,
    secrets: &net_kit::Secrets,
) -> AppResult<()> {
    let session = stored.session();
    emit(
        out,
        &status_of(
            app,
            &session,
            stored.email.clone(),
            password_stored(secrets),
        ),
    )?;
    Ok(())
}

fn import(app: &App, file: &str) -> AppResult<()> {
    let text = match file {
        "-" => {
            let mut buffer = String::new();
            std::io::stdin().read_to_string(&mut buffer)?;
            buffer
        }
        path => std::fs::read_to_string(path)?,
    };

    let imported = kmart_api::auth::session_from_netscape(&text);
    let admitted: Vec<Country> = Country::ALL
        .into_iter()
        .filter(|c| imported.admitted(*c))
        .collect();
    if admitted.is_empty() {
        return Err(kmart_api::Error::NoSession {
            detail: format!(
                ": {} carried no Kmart bot-check cookies. Sign in at kmart.co.nz or \
                 kmart.com.au first, then export while that tab is open",
                match file {
                    "-" => "standard input",
                    path => path,
                }
            ),
        }
        .into());
    }

    // Merged rather than replacing: an import for one country must not throw
    // away the other country's cookies or the signed-in token.
    let secrets = app.secrets();
    let mut stored = StoredSession::load(&secrets)?.unwrap_or_default();
    for country in &admitted {
        stored.cookies.insert(
            country.code().to_string(),
            kmart_api::auth::from_netscape(&text, *country),
        );
    }
    stored.save(&secrets)?;

    let mut out = app.out();
    emit(
        &mut out,
        &Imported {
            countries: admitted.iter().map(|c| c.to_string()).collect(),
        },
    )?;
    Ok(())
}

/// Keep a refresh token lifted out of a browser.
///
/// **Spent once, here, to check it.** The alternative -- storing whatever was
/// typed and letting the next command find out -- accepts a placeholder or a
/// half-copied token without complaint and then fails somewhere that reads as
/// a Kmart problem rather than a paste problem.
///
/// Auth0 refusing the token and the endpoint being unreachable are different
/// answers and are treated differently: the first refuses the import, the
/// second keeps it and says it could not be checked. Conflating them would
/// make a flaky connection look like a bad credential.
async fn token_import(app: &App, token: Option<String>) -> AppResult<()> {
    let token = match token {
        Some(token) => token,
        // Hidden, and off the shell history.
        None => prompt_password("Refresh token")?,
    };
    let token = token.trim();
    if token.is_empty() {
        return Err(AppError::usage("no token given"));
    }
    // The commonest paste mistake is the whole local-storage entry rather than
    // the field inside it, and the resulting failure is otherwise a baffling
    // `invalid_grant` from Auth0 much later.
    if token.starts_with('{') {
        return Err(AppError::usage(concat!(
            "that looks like the whole local-storage entry; ",
            "copy the value of its `refresh_token` field only"
        )));
    }

    let trace: kmart_api::auth::Trace<'_> = &|step: &str, detail: &str| {
        if app.env.debug {
            eprintln!("kmart: [{step}] {detail}");
        }
    };
    let checked = kmart_api::auth::refresh(&app.endpoints(), token, trace).await;

    let (tokens, verified) = match checked {
        // Auth0 kept its word. Store what it gave back, including the access
        // token, so the next command does not have to spend a second call.
        Ok(mut fresh) => {
            // Auth0 rotates the refresh token only sometimes; keeping the
            // original when it does not is the difference between a session
            // that renews forever and one that dies in fifteen minutes.
            if fresh.refresh.is_none() {
                fresh.refresh = Some(token.to_string());
            }
            (fresh, true)
        }
        // Auth0 answered, and said no. The token is the problem.
        Err(e @ kmart_api::Error::LoginRefused { .. }) => return Err(e.into()),
        Err(kmart_api::Error::Challenged { host }) => {
            return Err(kmart_api::Error::Challenged { host }.into())
        }
        // Nobody answered. Not evidence about the token either way, so it is
        // kept and the next command will find out.
        Err(_) => (kmart_api::Tokens::from_refresh(token), false),
    };

    let secrets = app.secrets();
    let mut stored = kmart_api::StoredSession::load(&secrets)?.unwrap_or_default();
    stored.tokens = Some(tokens);
    // A token pasted in came from a browser on one of the two storefronts,
    // and `--country` is the only statement of which.
    stored.auth_country = Some(app.country.code().to_string());
    stored.save(&secrets)?;

    let mut out = app.out();
    match verified {
        true => {
            writeln!(out, "Refresh token accepted by Kmart and kept.")?;
            writeln!(
                out,
                "{}",
                out.dim("It renews itself, so unlike the cookies this is a one-off.")
            )?;
        }
        false => {
            writeln!(
                out,
                "Refresh token kept, but Kmart could not be reached to check it."
            )?;
            writeln!(
                out,
                "{}",
                out.dim("Run `kmart auth status` once you are online.")
            )?;
        }
    }
    Ok(())
}

fn status(app: &App) -> AppResult<()> {
    let secrets = app.secrets();
    let stored = StoredSession::load(&secrets)?;
    let session = stored
        .as_ref()
        .map(StoredSession::session)
        .unwrap_or_default();
    let email = stored.as_ref().and_then(|s| s.email.clone());
    let password_stored = password_stored(&secrets);

    emit(
        &mut app.out(),
        &status_of(app, &session, email, password_stored),
    )?;
    Ok(())
}

fn status_of(
    app: &App,
    session: &Session,
    account: Option<String>,
    password_stored: bool,
) -> Status {
    Status {
        signed_in: session.signed_in(),
        account,
        // Left out entirely while nothing has been spent on the grant. A zero
        // there is not "it expired a moment ago", it is "no token has been
        // fetched yet", and a script that read the first would warn about a
        // sign-in that had just succeeded.
        expires_in: session
            .tokens()
            .filter(|t| !t.pending())
            .map(|t| t.expires_at.saturating_sub(net_kit::jwt::now_secs())),
        pending: session.tokens().is_some_and(kmart_api::Tokens::pending),
        renewable: session.tokens().is_some_and(|t| t.refresh.is_some()),
        admitted: Country::ALL
            .into_iter()
            .filter(|c| session.admitted(*c))
            .map(|c| c.to_string())
            .collect(),
        country: app.country.to_string(),
        password_stored,
    }
}

fn logout(app: &App) -> AppResult<()> {
    let secrets = app.secrets();
    // All of it, always. Leaving the password behind after a logout is the
    // kind of surprise that only shows up much later.
    let had_session = StoredSession::clear(&secrets)?;
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
    /// Seconds. The access token is a readable JWT, so this is a fact rather
    /// than an estimate. Absent when there is no access token to have one.
    #[serde(skip_serializing_if = "Option::is_none")]
    expires_in: Option<u64>,
    /// Whether the grant has yet to be spent on an access token, which is
    /// where a sign-in leaves it and is not a problem.
    pending: bool,
    /// Whether it renews without a password.
    renewable: bool,
    /// The countries whose gateway will answer, which is a separate question
    /// from being signed in.
    admitted: Vec<String>,
    country: String,
    password_stored: bool,
}

impl View for Status {
    fn text(&self, out: &mut Out) -> std::io::Result<()> {
        match self.signed_in {
            // Not `auth login`: Kmart's bot check blocks the password submit,
            // so pointing there would send someone somewhere that cannot work.
            false => writeln!(
                out,
                "{}. Run `kmart auth token` with a refresh token from a browser.",
                out.dim("Signed out")
            )?,
            true => {
                let who = self.account.as_deref().unwrap_or("Signed in");
                match self.expires_in {
                    // What a sign-in that just worked looks like: the grant is
                    // good and nothing has been spent on it. Saying "lapsed"
                    // of that would report a success as a fault.
                    _ if self.pending => {
                        writeln!(out, "{who}. The next command will fetch a token.")?
                    }
                    // A token that has run out is not a problem when it
                    // renews, so the two are said in one line rather than
                    // reported as a failure.
                    Some(0) if self.renewable => {
                        writeln!(out, "{who}. The token has lapsed and will renew.")?
                    }
                    Some(secs) => writeln!(
                        out,
                        "{who}, for another {}.",
                        human_duration(Duration::from_secs(secs))
                    )?,
                    None => writeln!(out, "{who}.")?,
                }
                if !self.renewable && !self.password_stored {
                    writeln!(
                        out,
                        "{}",
                        out.dim(
                            "No refresh token and no stored password, so this will \
                             need signing in again."
                        )
                    )?;
                }
            }
        }

        // The half people forget, and the one that expires first.
        match self.admitted.is_empty() {
            true => writeln!(
                out,
                "No bot-check cookies. Stock, stores and the account commands will be \
                 refused: run `kmart auth import <cookies.txt>`."
            )?,
            false => {
                writeln!(out, "Bot-check cookies for: {}.", self.admitted.join(", "))?;
                if !self.admitted.contains(&self.country) {
                    writeln!(
                        out,
                        "{}",
                        out.dim(&format!(
                            "None for {}, which is the country in use.",
                            self.country
                        ))
                    )?;
                }
            }
        }
        Ok(())
    }
}

#[derive(Serialize)]
struct Imported {
    countries: Vec<String>,
}

impl View for Imported {
    fn text(&self, out: &mut Out) -> std::io::Result<()> {
        writeln!(
            out,
            "Bot-check cookies imported for: {}.",
            self.countries.join(", ")
        )?;
        // The single most useful thing to know about them.
        writeln!(out, "{}", out.dim("They last about a day."))
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

#[cfg(test)]
mod tests {
    use super::*;
    use cli_kit::Format;

    fn render(status: &Status) -> String {
        let mut out = Out::buffer(Format::Text);
        emit(&mut out, status).unwrap();
        out.into_string()
    }

    fn status(signed_in: bool, admitted: Vec<&str>) -> Status {
        Status {
            signed_in,
            account: Some("shopper@example.test".into()),
            expires_in: Some(900),
            pending: false,
            renewable: true,
            admitted: admitted.into_iter().map(str::to_string).collect(),
            country: "nz".into(),
            password_stored: false,
        }
    }

    #[test]
    fn the_two_credentials_are_reported_separately() {
        // Signed in but not admitted is the normal state after a login, and
        // telling someone to sign in again would be the wrong advice.
        let text = render(&status(true, vec![]));
        assert!(text.contains("shopper@example.test"), "{text}");
        assert!(text.contains("auth import"), "{text}");
        assert!(!text.contains("auth login"), "{text}");

        // Admitted but not signed in is the other half.
        let text = render(&status(false, vec!["nz"]));
        assert!(text.contains("auth token"), "{text}");
        assert!(
            !text.contains("auth login"),
            "the password flow cannot work, so nothing may point at it: {text}"
        );
        assert!(text.contains("Bot-check cookies for: nz"), "{text}");
    }

    #[test]
    fn cookies_for_the_wrong_country_are_called_out() {
        let text = render(&status(true, vec!["au"]));
        assert!(text.contains("None for nz"), "{text}");
    }

    #[test]
    fn a_sign_in_that_just_worked_is_not_reported_as_lapsed() {
        // What `auth login` prints a second after it succeeds. The grant is
        // good and nothing has been spent on it, which is not the same state
        // as a token that ran out -- though both are `lapsed` underneath.
        let mut s = status(true, vec!["nz"]);
        s.pending = true;
        s.expires_in = None;
        let text = render(&s);
        assert!(text.contains("will fetch a token"), "{text}");
        assert!(!text.contains("lapsed"), "{text}");
    }

    #[test]
    fn a_lapsed_but_renewable_token_is_not_reported_as_a_problem() {
        let mut s = status(true, vec!["nz"]);
        s.expires_in = Some(0);
        let text = render(&s);
        assert!(text.contains("will renew"), "{text}");
    }

    #[test]
    fn no_refresh_token_and_no_password_is_the_case_worth_warning_about() {
        let mut s = status(true, vec!["nz"]);
        s.renewable = false;
        s.password_stored = false;
        assert!(render(&s).contains("need signing in again"));
    }
}
