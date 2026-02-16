mod cli;
mod config;
mod docker;
mod utils;

use anyhow::Result;
use clap::Parser;
use cli::Cli;

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging
    utils::logger::init()?;

    // Parse CLI arguments and execute
    let cli = Cli::parse();
    cli.execute().await
}
