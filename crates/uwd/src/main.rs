//! `uwd` — run the collector as a standalone process.
//!
//! Thin by design: everything is in the library, which the desktop app embeds.
//! A bug fixed here is fixed there.

use anyhow::Result;
use clap::Parser;
use uw_core::Config;

#[derive(Parser)]
#[command(name = "uwd", about = "Collect AI subscription usage and serve it to viewers")]
struct Cli {
    /// Override `[daemon] bind` from the config file.
    #[arg(long)]
    bind: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("UWD_LOG")
                .unwrap_or_else(|_| "uwd=info,uw_core=warn".into()),
        )
        .init();

    let cli = Cli::parse();
    uwd::start(Config::load()?, cli.bind)
        .await?
        .run_until_ctrl_c()
        .await
}
