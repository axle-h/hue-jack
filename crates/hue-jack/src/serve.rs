//! The daemon.
use crate::cli::GlobalArgs;
#[derive(Debug, clap::Args)]
pub struct ServeArgs {}
pub fn run(_g: &GlobalArgs, _args: ServeArgs) -> anyhow::Result<()> {
    anyhow::bail!("not implemented yet")
}
