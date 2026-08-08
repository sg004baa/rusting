use clap::Parser as _;

mod cli;

#[tokio::main]
async fn main() -> anyhow::Result<std::process::ExitCode> {
    cli::run(cli::Cli::parse()).await
}
