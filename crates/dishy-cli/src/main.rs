//! Headless recorder and debug CLI.
//!
//! The GTK app embeds `dishy-core` directly. This binary exists so the same
//! recorder can run on a server or Raspberry Pi, and so its behaviour can be
//! exercised without a display.

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use dishy_core::paths;
use dishy_core::runner::RecorderRunner;
use tracing::{info, warn};

#[derive(Debug, Parser)]
#[command(name = "dishy", about = "Dishylink recorder and diagnostics")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Print the bundled schema's request/response names.
    Schema,
    /// Read one status reply from the dish and print it as JSON.
    Status {
        /// Use the router's LAN endpoint instead of the dish's.
        #[arg(long)]
        router: bool,
    },
    /// Record telemetry into the historian's data directory.
    Record {
        /// Where the NDJSON stores live. Defaults to $HISTORIAN_DATA_DIR, then
        /// the app's ~/.config/Dishylink/data.
        #[arg(long)]
        data_dir: Option<PathBuf>,
        /// Poll once and exit, for a manual check.
        #[arg(long)]
        once: bool,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "dishy=info".into()),
        )
        .init();

    match Cli::parse().command {
        Command::Schema => {
            let schema = dishy_core::client::DishClient::new(dishy_core::Target::Dish)?;
            println!(
                "{} / {}",
                schema
                    .schema()
                    .pool()
                    .get_message_by_name("SpaceX.API.Device.Request")
                    .is_some(),
                schema
                    .schema()
                    .pool()
                    .get_message_by_name("SpaceX.API.Device.Response")
                    .is_some(),
            );
        }
        Command::Status { router } => {
            let target = if router {
                dishy_core::Target::Router
            } else {
                dishy_core::Target::Dish
            };
            let client = dishy_core::DishClient::new(target)?;
            let value = if router {
                client.get_router_status().await?
            } else {
                client.get_status().await?
            };
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
        Command::Record { data_dir, once } => {
            // SPEC-RECORDER-008: the same history the app records.
            let data_dir = data_dir.unwrap_or_else(|| {
                paths::data_dir(&paths::config_home(), paths::data_dir_override())
            });
            record(data_dir, once).await?;
        }
    }
    Ok(())
}

/// Poll the dish's history at the historian's existing cadence. The router's
/// status reply rides the same cycle, and only for the two live readings the
/// snapshot stamps onto samples — no new request is introduced.
///
/// The poll itself lives in `dishy_core::runner`, so this CLI and the GTK app
/// share one implementation rather than each owning a copy of it.
async fn record(data_dir: PathBuf, once: bool) -> anyhow::Result<()> {
    let mut runner = RecorderRunner::open(data_dir)?;
    info!("recording to {}", runner.data_dir().display());

    loop {
        let mut report = runner.tick(dishy_core::calendar::now_ms()).await;
        if let Some(poll) = report.history.take() {
            info!(
                "{} new sample(s), {} energy and {} latency minute(s), {} event(s)",
                poll.outcome.fresh_samples,
                poll.outcome.energy_minutes_written.len(),
                poll.outcome.latency_minutes_written.len(),
                poll.outcome.new_events
            );
        }
        if let Some(error) = report.history_error.take() {
            warn!("poll failed: {error}");
            if once {
                return Err(error.into());
            }
            if !sleep_or_stop().await {
                break;
            }
            continue;
        }

        if once || !sleep_or_stop().await {
            break;
        }
    }

    runner.write_snapshot()?;
    Ok(())
}

/// Sleep for the host tick, returning false if SIGINT/SIGTERM arrives. The tick
/// is one second; each poll inside it keeps its own approved cadence.
async fn sleep_or_stop() -> bool {
    tokio::select! {
        () = tokio::time::sleep(std::time::Duration::from_secs(1)) => true,
        _ = tokio::signal::ctrl_c() => false,
    }
}
