use anyhow::Result;
use clap::Parser;
use tucdp::{start, Args};

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    start(args.command).await
}
