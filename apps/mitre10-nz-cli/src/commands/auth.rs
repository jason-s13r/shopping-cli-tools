//! `auth` -- credentials, which cost a password once and never again.
//!
//! There is no captcha anywhere in this flow, so unlike the Briscoes tool
//! there is no browser to drive: an email and a password are the whole story.
//! What is stored is the refresh token, never the password.

use cli_kit::emit;
use mitre10_api::{auth, StoredSession};

use crate::app::App;
use crate::cli::AuthAction;
use crate::error::{AppError, AppResult};
use crate::views::AuthStatus;

pub async fn run(app: &App, action: AuthAction) -> AppResult<()> {
    match action {
        AuthAction::Login { email, stdin } => login(app, email, stdin).await,
        AuthAction::Status => status(app),
        AuthAction::Refresh => refresh(app).await,
        AuthAction::Logout => logout(app).await,
    }
}

async fn login(app: &App, email: Option<String>, stdin: bool) -> AppResult<()> {
    let email = match email {
        Some(email) => email,
        None => cli_kit::prompt("Email").map_err(|e| AppError::usage(e.to_string()))?,
    };
    let password = if stdin {
        cli_kit::prompt_or_stdin("Password").map_err(|e| AppError::usage(e.to_string()))?
    } else {
        cli_kit::prompt_password("Password").map_err(|e| AppError::usage(e.to_string()))?
    };
    if email.trim().is_empty() || password.is_empty() {
        return Err(AppError::usage("an email and a password are both needed"));
    }

    let debug = app.env.debug;
    let tokens = auth::login(
        mitre10_api::client_spec_for(app.emulation),
        &app.endpoints(),
        email.trim(),
        &password,
        &move |step: &str, detail: &str| {
            if debug {
                eprintln!("mitre10: {step} {detail}");
            }
        },
    )
    .await?;

    // Adopted through the client so it is filed the same way a renewal is,
    // rather than by a second path that could drift from it.
    app.client()?.adopt(tokens);
    println!("Signed in as {}.", email.trim());
    Ok(())
}

fn status(app: &App) -> AppResult<()> {
    let secrets = app.secrets();
    let session = StoredSession::load(&secrets)?
        .map(|s| s.session())
        .unwrap_or_default();

    let mut out = app.out();
    emit(
        &mut out,
        &AuthStatus {
            signed_in: session.can_refresh() || session.access_token.is_some(),
            email: session.email.clone(),
            // Only while the token is good for the next request. A lapsed one
            // is the same news as no token at all: the next command mints one.
            expires_in: session
                .token_fresh()
                .then_some(session.expires_at)
                .flatten()
                .and_then(|at| at.checked_sub(net_kit::jwt::now_ms()))
                .map(|left| left / 1000),
            can_refresh: session.can_refresh(),
            backend: secrets.backend().describe().to_string(),
        },
    )?;
    Ok(())
}

async fn refresh(app: &App) -> AppResult<()> {
    let client = app.client()?;
    client.renew().await?;
    println!("Renewed.");
    Ok(())
}

async fn logout(app: &App) -> AppResult<()> {
    let client = app.client()?;
    if !client.is_signed_in() {
        println!("Not signed in.");
        return Ok(());
    }
    client.sign_out().await?;
    println!("Signed out.");
    Ok(())
}
