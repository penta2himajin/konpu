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
        /// Path to konpu.toml (default: ./konpu.toml if present)
        #[arg(long)]
        config: Option<String>,
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Check { path, config } => {
            use konpu::analyze::template;
            use konpu::domain::konpu::Severity;
            let p = std::path::PathBuf::from(path);
            let config_path = config
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("konpu.toml"));
            let resolved = template::load(&config_path);
            let diagnostics = konpu::analyze::analyze_with_config(&p, &resolved);
            if diagnostics.is_empty() {
                println!("konpu check: no violations");
                return;
            }
            for d in &diagnostics {
                println!(
                    "{}:{}: {:?} {:?} target={:?}",
                    d.path.display(),
                    d.line,
                    d.diag.severity,
                    d.diag.rule,
                    d.diag.declaration.targetStructure
                );
            }
            let has_error = diagnostics.iter().any(|d| d.diag.severity == Severity::Error);
            std::process::exit(if has_error { 1 } else { 0 });
        }
    }
}
