//! `uw` — read every provider's remaining headroom from one command.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use uw_core::auth::TokenSource;
use uw_core::model::{AuthKind, MeterKind, Provider, Severity, Status};
use uw_core::providers::{claude::Claude, codex::Codex, Adapter, AuthPreference};
use uw_core::Config;

#[derive(Parser)]
#[command(name = "uw", about = "Watch remaining usage across your AI subscriptions")]
struct Cli {
    #[command(subcommand)]
    command: Option<Command>,

    /// Emit JSON instead of a table.
    #[arg(long, global = true)]
    json: bool,
}

#[derive(Subcommand)]
enum Command {
    /// Show current usage (the default when no subcommand is given).
    Status,
    /// Manage per-provider authentication.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
}

#[derive(Subcommand)]
enum AuthCommand {
    /// Sign in to a provider with its own OAuth grant.
    Login { provider: String },
    /// Take over the vendor CLI's current grant as our own.
    ///
    /// Copies the CLI's refresh token into our store so we can rotate it
    /// ourselves. The authorize step is gated by bot attestation we cannot (and
    /// should not) synthesize, but the token endpoint is plain OAuth — so an
    /// already-issued refresh token is all we need, and it works anywhere,
    /// Android included.
    ///
    /// Re-run the vendor login afterwards so the CLI gets its own fresh grant.
    Adopt { provider: String },
    /// Store a long-lived token pasted in by hand.
    ///
    /// For Claude, run `claude setup-token` and paste what it prints — a
    /// one-year OAuth token, and the path Anthropic documents for environments
    /// without an interactive browser.
    Token { provider: String },
    /// Forget a provider's stored credential.
    Logout { provider: String },
    /// Show how each provider is authenticated.
    Status,
    /// Switch a provider between borrowing the CLI's token and its own grant.
    Mode {
        provider: String,
        #[arg(value_enum)]
        mode: ModeArg,
    },
}

#[derive(Clone, Copy, ValueEnum)]
enum ModeArg {
    /// Borrow the vendor CLI's token, read-only. Needs that CLI installed.
    Delegated,
    /// Our own OAuth grant. Works without the vendor CLI (e.g. on Android).
    Own,
    /// A long-lived token pasted in by hand (`claude setup-token`).
    Token,
}

impl From<ModeArg> for AuthPreference {
    fn from(m: ModeArg) -> Self {
        match m {
            ModeArg::Delegated => AuthPreference::Delegated,
            ModeArg::Own => AuthPreference::Own,
            ModeArg::Token => AuthPreference::Token,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        None | Some(Command::Status) => status(cli.json).await,
        Some(Command::Auth { command }) => auth(command).await,
    }
}

async fn status(as_json: bool) -> Result<()> {
    let cfg = Config::load()?;
    let http = uw_core::http_client();

    // Concurrent, so one slow provider never delays the others.
    let providers = uw_core::collect::poll_all(&cfg, &http).await;

    if as_json {
        let snap = uw_core::Snapshot {
            generated_at: chrono::Utc::now(),
            providers,
        };
        println!("{}", serde_json::to_string_pretty(&snap)?);
    } else {
        print_table(&providers);
    }
    Ok(())
}

async fn auth(command: AuthCommand) -> Result<()> {
    match command {
        AuthCommand::Login { provider } => login(&provider).await,
        AuthCommand::Token { provider } => store_token(&provider).await,
        AuthCommand::Adopt { provider } => adopt(&provider).await,
        AuthCommand::Logout { provider } => {
            let cfg = Config::load()?;
            let source = source_for(&provider, &cfg)?;
            source.logout().await?;
            println!("Signed out of {provider}.");
            Ok(())
        }
        AuthCommand::Status => {
            let cfg = Config::load()?;
            for (id, label) in [("claude", "Claude Code"), ("codex", "OpenAI Codex")] {
                let pref = cfg.auth_pref(id);
                let mode = match pref {
                    AuthPreference::Own => "own grant",
                    AuthPreference::Delegated => "delegated (borrows the CLI)",
                    AuthPreference::Token => "long-lived token",
                };
                let state = match source_for(id, &cfg) {
                    Ok(s) => match s.access_token().await {
                        Ok(c) if c.is_expired() => "expired".to_string(),
                        Ok(_) => "ok".to_string(),
                        Err(e) => format!("{e:#}"),
                    },
                    Err(e) => format!("{e:#}"),
                };
                println!("{label:<16} {mode:<28} {state}");
            }
            Ok(())
        }
        AuthCommand::Mode { provider, mode } => {
            let mut cfg = Config::load()?;
            let pref: AuthPreference = mode.into();
            cfg.set_auth_pref(&provider, pref);
            cfg.save()?;
            println!(
                "{provider} now uses {}.",
                match pref {
                    AuthPreference::Own => "its own OAuth grant",
                    AuthPreference::Delegated => "the vendor CLI's token (read-only)",
                    AuthPreference::Token => "a pasted long-lived token",
                }
            );
            if pref == AuthPreference::Own {
                println!("Run `uw auth login {provider}` to sign in.");
            }
            Ok(())
        }
    }
}

async fn login(provider: &str) -> Result<()> {
    let mut cfg = Config::load()?;

    // Logging in only makes sense for an own grant, so flip the toggle rather
    // than failing with "this provider is in delegated mode".
    if cfg.auth_pref(provider) != AuthPreference::Own {
        cfg.set_auth_pref(provider, AuthPreference::Own);
        cfg.save()?;
        println!("Switched {provider} to its own OAuth grant.\n");
    }

    let source = source_for(provider, &cfg)?;
    let mut cred = source.login(&TerminalLogin).await?;

    // Best effort: this only decorates the tile with a plan name, and the
    // credential itself is already stored and usable.
    let http = uw_core::reqwest::Client::new();
    match uw_core::providers::enrich(provider, &http, &mut cred).await {
        Ok(()) => source.store(cred).await?,
        Err(e) => eprintln!("note: signed in, but could not read the account profile: {e:#}"),
    }

    println!("\nSigned in to {provider}.");
    Ok(())
}

async fn adopt(provider: &str) -> Result<()> {
    use uw_core::auth::TokenStore;
    use uw_core::providers::{claude, codex};

    let (path, cred) = match provider {
        "claude" => {
            let p = Claude
                .delegated_path()
                .context("cannot locate the Claude credentials file")?;
            (p.clone(), claude::read_full_credential(&p)?)
        }
        "codex" => {
            let p = Codex
                .delegated_path()
                .context("cannot locate the Codex auth file")?;
            (p.clone(), codex::read_full_credential(&p)?)
        }
        other => bail!("unknown provider `{other}` (known: claude, codex)"),
    };

    TokenStore::save(provider, &cred)?;

    let mut cfg = Config::load()?;
    cfg.set_auth_pref(provider, AuthPreference::Own);
    cfg.save()?;

    println!("Adopted the grant from {}.", path.display());
    println!("{provider} now refreshes that token independently.\n");
    println!("IMPORTANT: run the vendor login again now, so the CLI gets a grant of");
    println!("its own and the two stop sharing one refresh token:\n");
    println!(
        "  {}\n",
        if provider == "claude" { "claude auth login" } else { "codex login" }
    );
    println!("Then run `uw` to confirm the adopted token still works.");
    Ok(())
}

async fn store_token(provider: &str) -> Result<()> {
    use std::io::Write;
    use uw_core::auth::{Credential, TokenStore};

    if provider == "claude" {
        println!("Run `claude setup-token` in another terminal, then paste the token here.");
    }
    print!("Token: ");
    std::io::stdout().flush()?;

    let mut line = String::new();
    std::io::stdin().read_line(&mut line)?;
    let token = line.trim().to_string();
    if token.is_empty() {
        bail!("no token entered");
    }

    // No expiry recorded: these are long-lived by design and carry no refresh
    // token, so there is nothing to refresh and nothing to rotate.
    TokenStore::save(
        &format!("{provider}-token"),
        &Credential {
            access_token: token,
            refresh_token: None,
            expires_at: None,
            extra: Default::default(),
        },
    )?;

    let mut cfg = Config::load()?;
    cfg.set_auth_pref(provider, AuthPreference::Token);
    cfg.save()?;

    println!("Stored. {provider} now uses that token.");
    Ok(())
}

struct TerminalLogin;

impl uw_core::auth::LoginUi for TerminalLogin {
    fn open(&self, url: &str) -> Result<()> {
        println!("Open this URL to sign in:\n\n  {url}\n");
        // Under WSL there is often no browser to launch; the printed URL above
        // is the real path, so a failure here is not an error.
        let _ = open::that(url);
        Ok(())
    }

    fn read_code(&self) -> Result<String> {
        use std::io::Write;
        print!("Paste the code shown on the page: ");
        std::io::stdout().flush()?;

        let mut line = String::new();
        std::io::stdin().read_line(&mut line)?;
        if line.trim().is_empty() {
            bail!("no code entered");
        }
        Ok(line)
    }

    /// Reads stdin on a plain thread so it can be raced against the loopback
    /// listener. If the browser reaches the listener first, this thread is
    /// simply abandoned along with the process.
    fn paste_channel(&self) -> Option<tokio::sync::oneshot::Receiver<String>> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            println!("Waiting for the browser to redirect back...");
            println!("If the page shows a code instead, paste it here and press Enter.");
            let mut line = String::new();
            if std::io::stdin().read_line(&mut line).is_ok() && !line.trim().is_empty() {
                let _ = tx.send(line);
            }
        });
        Some(rx)
    }
}

fn source_for(provider: &str, cfg: &Config) -> Result<TokenSource> {
    let pref = cfg.auth_pref(provider);
    match provider {
        "claude" => Claude.token_source(pref),
        "codex" => Codex.token_source(pref),
        other => bail!("unknown provider `{other}` (known: claude, codex)"),
    }
}

fn print_table(providers: &[Provider]) {
    for p in providers {
        let plan = p.plan.as_deref().unwrap_or("—");
        let auth = match p.auth {
            AuthKind::OwnGrant => "own",
            AuthKind::Delegated => "delegated",
            AuthKind::ApiKey => "api-key",
            AuthKind::None => "none",
        };
        println!("\n{}  ({plan}, {auth})", p.label);

        match &p.status {
            Status::Error { message } => {
                println!("  ! {message}");
                continue;
            }
            Status::Unavailable { reason } => {
                println!("  – unavailable: {reason}");
                continue;
            }
            Status::Stale { since } => println!("  (stale since {})", since.to_rfc3339()),
            Status::Ok => {}
        }

        if p.meters.is_empty() {
            println!("  (no meters reported)");
        }

        for m in &p.meters {
            match &m.kind {
                MeterKind::Window {
                    used_pct,
                    resets_at,
                    ..
                } => {
                    let reset = resets_at
                        .map(|r| format!("resets in {}", human_delta(r)))
                        .unwrap_or_default();
                    println!(
                        "  {:<16} {} {:>3.0}%  {}",
                        m.label,
                        bar(*used_pct),
                        used_pct,
                        mark(m.severity, &reset)
                    );
                }
                MeterKind::Balance {
                    amount,
                    currency,
                    unlimited,
                    ..
                } => {
                    let v = if *unlimited {
                        "unlimited".to_string()
                    } else {
                        format!("{amount:.2} {currency}")
                    };
                    println!("  {:<16} {}", m.label, mark(m.severity, &v));
                }
                MeterKind::Spend {
                    amount, currency, ..
                } => println!("  {:<16} {amount:.2} {currency} spent", m.label),
            }
        }
    }
}

fn bar(pct: f32) -> String {
    let filled = ((pct / 5.0).round() as usize).min(20);
    format!("[{}{}]", "█".repeat(filled), "░".repeat(20 - filled))
}

fn mark(sev: Severity, text: &str) -> String {
    match sev {
        Severity::Normal => text.to_string(),
        Severity::Warning => format!("{text}  ⚠"),
        Severity::Critical => format!("{text}  ‼"),
    }
}

fn human_delta(when: chrono::DateTime<chrono::Utc>) -> String {
    let secs = (when - chrono::Utc::now()).num_seconds();
    if secs <= 0 {
        return "now".into();
    }
    let (d, h, m) = (secs / 86400, (secs % 86400) / 3600, (secs % 3600) / 60);
    match (d, h) {
        (0, 0) => format!("{m}m"),
        (0, _) => format!("{h}h {m}m"),
        _ => format!("{d}d {h}h"),
    }
}
