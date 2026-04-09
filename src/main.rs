use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "konpu", version, about = "Algebraic complexity linter")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Run static analysis on annotated code
    Check {
        /// Path to analyze
        path: String,
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Check { path } => {
            println!("konpu check: {path}");
            println!("not yet implemented");
        }
    }
}
