use clap::Parser as _;

mod cli;
mod postman;

#[tokio::main]
async fn main() -> anyhow::Result<std::process::ExitCode> {
    cli::run(cli::Cli::parse()).await
}
