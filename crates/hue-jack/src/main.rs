use clap::Parser;
use hue_jack::cli::Cli;

fn main() -> anyhow::Result<()> {
    let _cli = Cli::parse();
    Ok(())
}
