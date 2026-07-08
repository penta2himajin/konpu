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
    /// Generate law-test skeletons for annotated declarations
    Scaffold {
        /// Path to analyze (file or directory)
        path: String,
        /// Path to konpu.toml (default: ./konpu.toml if present)
        #[arg(long)]
        config: Option<String>,
        /// Write the generated files (default: print to stdout only)
        #[arg(long)]
        write: bool,
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
        Commands::Scaffold { path, config, write } => {
            use konpu::analyze::scaffold;
            use konpu::analyze::template;
            let p = std::path::PathBuf::from(path);
            let config_path = config
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("konpu.toml"));
            let resolved = template::load(&config_path);
            let files = scaffold::scaffold_path(&p, &resolved);
            if files.is_empty() {
                println!("konpu scaffold: no annotated declarations found");
                return;
            }
            let mut decl_total = 0;
            let mut test_total = 0;
            for f in &files {
                decl_total += f.decl_count;
                test_total += f.test_count;
                if write {
                    if let Err(e) = std::fs::write(&f.path, &f.contents) {
                        eprintln!("konpu scaffold: failed to write {}: {e}", f.path.display());
                        std::process::exit(1);
                    }
                    println!("wrote {} ({} tests)", f.path.display(), f.test_count);
                } else {
                    println!("// would write {} ({} tests)", f.path.display(), f.test_count);
                    println!("{}", f.contents);
                }
            }
            println!(
                "konpu scaffold: {} file(s), {} declaration(s), {} test(s){}",
                files.len(),
                decl_total,
                test_total,
                if write { " written" } else { " (dry-run, pass --write to emit)" }
            );
        }
    }
}
