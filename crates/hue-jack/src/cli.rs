use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "hue-jack", version, about = "Music to Philips Hue appliance")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Run the daemon.
    Serve,
}
