//! `uw` — read every provider's remaining headroom from one command.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use uw_core::auth::TokenSource;
use uw_core::model::{AuthKind, MeterKind, Provider, Severity, Status};
use uw_core::providers::{Any, AuthPreference};
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
    /// Add and remove the providers being watched.
    Provider {
        #[command(subcommand)]
        command: ProviderCommand,
    },
}

#[derive(Subcommand)]
enum ProviderCommand {
    /// Show which providers are being watched, and what else could be.
    List,
    /// Start watching a provider.
    Add {
        provider: String,
        /// How to authenticate. Defaults to whatever the provider recommends.
        #[arg(value_enum)]
        mode: Option<ModeArg>,
    },
    /// Stop watching a provider and delete its stored credential.
    #[command(alias = "rm")]
    Remove { provider: String },
}

#[derive(Subcommand)]
enum AuthCommand {
    /// Sign in to a provider with its own OAuth grant.
    Login { provider: String },
    /// Take over the vendor CLI's stored credential as our own.
    ///
    /// For Claude and Codex this copies the CLI's refresh token so we can rotate
    /// it ourselves, and you must re-run the vendor login afterwards or the two
    /// will fight over one rotating token. For opencode it copies a static API
    /// key, which is only a copy — the CLI is unaffected.
    ///
    /// Either way the result works where the vendor CLI does not exist, which
    /// is what a phone needs.
    Adopt { provider: String },
    /// Store a long-lived token pasted in by hand.
    ///
    /// For Claude, run `claude setup-token` and paste what it prints — a
    /// one-year OAuth token, and the path Anthropic documents for environments
    /// without an interactive browser. For opencode and OpenRouter this is just
    /// an API key from the provider's web console.
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
    /// A long-lived token or API key pasted in by hand.
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
        Some(Command::Provider { command }) => provider(command).await,
    }
}

async fn provider(command: ProviderCommand) -> Result<()> {
    match command {
        ProviderCommand::List => {
            let cfg = Config::load()?;
            for adapter in Any::all() {
                let info = adapter.info();
                match cfg.providers.get(adapter.id()) {
                    Some(p) => {
                        let signed_in = match adapter.has_credential(p.auth) {
                            Ok(true) => "signed in",
                            Ok(false) => "not signed in",
                            Err(_) => "unusable here",
                        };
                        println!("{:<12} {:<16} {signed_in}", adapter.id(), describe(p.auth));
                    }
                    None => println!("{:<12} {:<16} {}", adapter.id(), "—", info.summary),
                }
            }
            Ok(())
        }
        ProviderCommand::Add { provider, mode } => {
            let adapter = adapter_for(&provider)?;
            let pref = mode.map(Into::into).unwrap_or_else(|| adapter.default_auth());

            // Checked before the write: a mode this provider cannot support
            // would otherwise be saved and only surface later as an error tile.
            adapter.token_source(pref)?;

            let mut cfg = Config::load()?;
            let added = cfg.add(&provider, pref);
            cfg.save()?;

            println!(
                "{} {provider}, using {}.",
                if added { "Added" } else { "Updated" },
                describe(pref)
            );
            if pref == AuthPreference::Own && !adapter.has_credential(pref)? {
                println!("Run `uw auth login {provider}` to sign in.");
            }
            if pref == AuthPreference::Token && !adapter.has_credential(pref)? {
                println!("Run `uw auth token {provider}` to store a key.");
            }
            Ok(())
        }
        ProviderCommand::Remove { provider } => {
            let adapter = adapter_for(&provider)?;
            let mut cfg = Config::load()?;
            if !cfg.remove(&provider) {
                println!("{provider} was not being watched.");
                return Ok(());
            }
            cfg.save()?;
            // Config first, credential second: the reverse order can leave a
            // provider still polled with its token gone, which is an error the
            // user cannot clear.
            adapter.forget_credentials()?;
            println!("Removed {provider} and deleted its stored credential.");
            Ok(())
        }
    }
}

fn describe(pref: AuthPreference) -> &'static str {
    match pref {
        AuthPreference::Own => "own grant",
        AuthPreference::Delegated => "delegated",
        AuthPreference::Token => "pasted token",
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
    } else if providers.is_empty() {
        // Not an error, and not a bug: a fresh install watches nothing until
        // you say what to watch. Silence here reads as breakage.
        println!("No providers are being watched yet.\n");
        println!("  uw provider list          see what is available");
        println!("  uw provider add claude    start watching one");
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
            // Driven off the registry so a new adapter shows up here without
            // anyone remembering to add it.
            for adapter in Any::all() {
                // Providers that were never added are listed rather than
                // hidden: "why is Codex missing" is a question worth answering
                // in the same place as "why is Codex failing".
                if !cfg.is_added(adapter.id()) {
                    println!(
                        "{:<16} {:<28} not watched — `uw provider add {}`",
                        adapter.label(),
                        "—",
                        adapter.id()
                    );
                    continue;
                }
                let pref = adapter.auth_pref(&cfg);
                let mode = match pref {
                    AuthPreference::Own => "own grant",
                    AuthPreference::Delegated => "delegated (borrows the CLI)",
                    AuthPreference::Token => "long-lived token",
                };
                let state = match adapter.token_source(pref) {
                    Ok(s) => match s.access_token().await {
                        Ok(c) if c.is_expired() => "expired".to_string(),
                        Ok(_) => "ok".to_string(),
                        Err(e) => format!("{e:#}"),
                    },
                    Err(e) => format!("{e:#}"),
                };
                println!("{:<16} {mode:<28} {state}", adapter.label());
            }
            Ok(())
        }
        AuthCommand::Mode { provider, mode } => {
            let adapter = adapter_for(&provider)?;
            let pref: AuthPreference = mode.into();

            // Checked before the write, not after. A mode this provider cannot
            // support would otherwise be saved and only surface later, as an
            // error tile in the panel with no hint of which command caused it.
            adapter.token_source(pref)?;

            let mut cfg = Config::load()?;
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
    let adapter = adapter_for(provider)?;
    let mut cfg = Config::load()?;

    // A provider with no OAuth flow at all must say so before we start
    // rewriting config on its behalf.
    adapter.token_source(AuthPreference::Own)?;

    // Logging in only makes sense for an own grant, so flip the toggle rather
    // than failing with "this provider is in delegated mode" — and add the
    // provider if it was not being watched at all, because signing in to
    // something is how you ask for it.
    //
    // The `is_added` check is not redundant with the mode check: a provider
    // whose own default is already `own` (OpenRouter has no CLI to borrow from)
    // would pass the second and still never be written to the config file.
    if !cfg.is_added(provider) || adapter.auth_pref(&cfg) != AuthPreference::Own {
        let added = cfg.add(provider, AuthPreference::Own);
        cfg.save()?;
        if added {
            println!("Watching {provider}, using its own OAuth grant.\n");
        } else {
            println!("Switched {provider} to its own OAuth grant.\n");
        }
    }

    let source = adapter.token_source(AuthPreference::Own)?;
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

    let adapter = adapter_for(provider)?;
    let Some(target) = adapter.adopt_as() else {
        bail!(
            "`{provider}` has no vendor CLI credential to adopt — run \
             `uw auth login {provider}` instead"
        );
    };

    let (path, cred) = adapter.read_full_credential()?;

    // An adopted OAuth grant becomes ours and gets refreshed; an adopted API
    // key is only a copy, and lives under the pasted-token entry so the two
    // never fight over one slot.
    let entry = match target {
        AuthPreference::Token => adapter.token_entry(),
        _ => provider.to_string(),
    };
    TokenStore::save(&entry, &cred)?;

    let mut cfg = Config::load()?;
    cfg.set_auth_pref(provider, target);
    cfg.save()?;

    println!("Adopted the credential from {}.", path.display());

    match adapter.relogin_hint() {
        // Claude and Codex rotate refresh tokens: until the vendor CLI gets a
        // grant of its own, whichever of us refreshes first signs the other out.
        Some(cmd) => {
            println!("{provider} now refreshes that token independently.\n");
            println!("IMPORTANT: run the vendor login again now, so the CLI gets a grant of");
            println!("its own and the two stop sharing one refresh token:\n");
            println!("  {cmd}\n");
        }
        // A static API key. Copying it changes nothing for the vendor CLI, and
        // telling the user to sign in again would be busywork.
        None => println!(
            "That is a static API key, so the opencode CLI is unaffected — nothing \
             to sign in to again.\n"
        ),
    }
    println!("Run `uw` to confirm the adopted credential still works.");
    Ok(())
}

async fn store_token(provider: &str) -> Result<()> {
    use std::io::Write;
    use uw_core::auth::{Credential, TokenStore};

    let adapter = adapter_for(provider)?;

    // The prompt comes from the adapter's manifest rather than a `match` here,
    // so the terminal and the config screen tell you to look in the same place.
    let prompt = adapter
        .info()
        .methods
        .into_iter()
        .find_map(|m| m.token)
        .with_context(|| {
            format!("`{provider}` cannot use a pasted token — run `uw auth login {provider}`")
        })?;

    println!("{}", prompt.help);
    if let Some(url) = &prompt.url {
        println!("{url}");
    }
    print!("{}: ", prompt.label);
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
        &adapter.token_entry(),
        &Credential {
            access_token: token,
            refresh_token: None,
            expires_at: None,
            extra: Default::default(),
        },
    )?;

    let mut cfg = Config::load()?;
    cfg.add(provider, AuthPreference::Token);
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

fn adapter_for(provider: &str) -> Result<Any> {
    Any::by_id(provider).with_context(|| {
        format!(
            "unknown provider `{provider}` (known: {})",
            Any::known_ids()
        )
    })
}

fn source_for(provider: &str, cfg: &Config) -> Result<TokenSource> {
    let adapter = adapter_for(provider)?;
    adapter.token_source(adapter.auth_pref(cfg))
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
