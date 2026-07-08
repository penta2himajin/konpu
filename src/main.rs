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
            use konpu::analyze::analyze_path;
            use konpu::domain::konpu::Severity;
            let p = std::path::PathBuf::from(path);
            let diagnostics = analyze_path(&p);
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
