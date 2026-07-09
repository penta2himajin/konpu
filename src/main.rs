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
        /// Path to baseline file (default: .konpu/baseline.json if present).
        /// When set, only diagnostics NOT in the baseline are reported.
        #[arg(long)]
        baseline: Option<String>,
        /// Also run call-graph-powered preserve checks (needs rust-analyzer;
        /// only effective when built with --features call-graph).
        #[arg(long)]
        call_graph: bool,
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
    /// Record all current violations into a baseline file
    Baseline {
        /// Path to analyze
        path: String,
        /// Path to konpu.toml (default: ./konpu.toml if present)
        #[arg(long)]
        config: Option<String>,
        /// Output path for the baseline (default: .konpu/baseline.json)
        #[arg(long)]
        out: Option<String>,
    },
    /// Print a summary: diagnostics, ignores, declarations
    Report {
        /// Path to analyze
        path: String,
        /// Path to konpu.toml (default: ./konpu.toml if present)
        #[arg(long)]
        config: Option<String>,
    },
    /// Build the call graph (via rust-analyzer/SCIP) and report cycles + hubs
    #[cfg(feature = "call-graph")]
    Callgraph {
        /// Project directory to index (default: run rust-analyzer scip here)
        path: String,
        /// Use a pre-generated SCIP index instead of running rust-analyzer
        #[arg(long)]
        scip: Option<String>,
        /// Dispatch resolution precision: rta (default) or cha
        #[arg(long, default_value = "rta")]
        precision: String,
        /// Report functions whose fan-in or fan-out is at least this (default 8)
        #[arg(long, default_value_t = 8)]
        hub_threshold: usize,
    },
}

/// Run call-graph-powered preserve checks and print findings.
/// Returns true if any finding is Error severity (to drive the exit code).
#[cfg(feature = "call-graph")]
fn run_call_graph_preserve(
    path: &std::path::Path,
    config: &konpu::analyze::template::ResolvedConfig,
) -> bool {
    use konpu::analyze::{call_graph, preserve_cg};
    use konpu::domain::konpu::Severity;
    let facts = match call_graph::facts_from_project(path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("konpu check --call-graph: {e}");
            return false;
        }
    };
    let result = konpu::analyze::analyze_full(path, config);
    let sigs = call_graph::fn_signatures(path);
    let findings =
        preserve_cg::check_preserve(&result.declarations, &result.law_tests, config, &facts, &sigs, path);
    if findings.is_empty() {
        println!("konpu preserve: no call-graph violations");
        return false;
    }
    let mut has_error = false;
    for f in &findings {
        println!(
            "{}:{}: {:?} preserve[{:?}] `{}` — {}",
            f.path.display(),
            f.line,
            f.severity,
            f.kind,
            f.function,
            f.reason
        );
        if f.severity == Severity::Error {
            has_error = true;
        }
    }
    has_error
}

#[cfg(not(feature = "call-graph"))]
fn run_call_graph_preserve(
    _path: &std::path::Path,
    _config: &konpu::analyze::template::ResolvedConfig,
) -> bool {
    eprintln!("konpu check --call-graph: rebuild konpu with --features call-graph");
    false
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Check { path, config, baseline, call_graph } => {
            use konpu::analyze::baseline;
            use konpu::analyze::template;
            use konpu::domain::konpu::Severity;
            let p = std::path::PathBuf::from(path);
            let config_path = config
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("konpu.toml"));
            let resolved = template::load(&config_path);
            let diagnostics = konpu::analyze::analyze_with_config(&p, &resolved);
            let baseline_path = baseline
                .map(std::path::PathBuf::from)
                .unwrap_or_else(baseline::default_path);
            let bl = baseline::load(&baseline_path);
            let (diagnostics, _hidden) = if bl.is_empty() {
                (diagnostics, 0usize)
            } else {
                let new_count = diagnostics
                    .iter()
                    .filter(|d| {
                        !bl.contains(&baseline::BaselineEntry::from_diag(d))
                    })
                    .count();
                let new_diags = baseline::filter_new(diagnostics, &bl);
                (new_diags, new_count)
            };
            if diagnostics.is_empty() {
                println!("konpu check: no violations");
            } else {
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
            }
            let mut has_error = diagnostics.iter().any(|d| d.diag.severity == Severity::Error);
            if call_graph {
                has_error |= run_call_graph_preserve(&p, &resolved);
            }
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
        Commands::Baseline { path, config, out } => {
            use konpu::analyze::baseline;
            use konpu::analyze::template;
            let p = std::path::PathBuf::from(path);
            let config_path = config
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("konpu.toml"));
            let resolved = template::load(&config_path);
            let diagnostics = konpu::analyze::analyze_with_config(&p, &resolved);
            let out_path = out.map(std::path::PathBuf::from).unwrap_or_else(baseline::default_path);
            let entries = baseline::entries_from(&diagnostics);
            match baseline::save(&out_path, &entries) {
                Ok(()) => {
                    println!(
                        "konpu baseline: {} entry(s) written to {}",
                        entries.len(),
                        out_path.display()
                    );
                }
                Err(e) => {
                    eprintln!("konpu baseline: failed to write {}: {e}", out_path.display());
                    std::process::exit(1);
                }
            }
        }
        Commands::Report { path, config } => {
            use konpu::analyze::template;
            use konpu::domain::konpu::Severity;
            use std::collections::BTreeMap;
            let p = std::path::PathBuf::from(path);
            let config_path = config
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|| std::path::PathBuf::from("konpu.toml"));
            let resolved = template::load(&config_path);
            let result = konpu::analyze::analyze_full(&p, &resolved);
            let mut by_sev: BTreeMap<Severity, usize> = BTreeMap::new();
            let mut by_rule: BTreeMap<String, usize> = BTreeMap::new();
            for d in &result.diagnostics {
                *by_sev.entry(d.diag.severity.clone()).or_insert(0) += 1;
                *by_rule.entry(format!("{:?}", d.diag.rule)).or_insert(0) += 1;
            }
            let mut by_ignore_reason: BTreeMap<String, usize> = BTreeMap::new();
            for ig in &result.ignores {
                let key = format!("{:?}", ig.reason);
                *by_ignore_reason.entry(key).or_insert(0) += 1;
            }
            println!("== konpu report ==");
            println!("path: {}", p.display());
            println!(
                "config: {} layer(s), defaults_max_propagation: {:?}",
                resolved.layers.len(),
                resolved.defaults_max
            );
            println!("declarations: {}", result.declarations.len());
            println!("impls: {}", result.impls.len());
            println!("law tests: {}", result.law_tests.len());
            println!("diagnostics: {}", result.diagnostics.len());
            for (s, n) in &by_sev {
                println!("  {s:?}: {n}");
            }
            for (r, n) in &by_rule {
                println!("  rule {r}: {n}");
            }
            println!("ignores: {}", result.ignores.len());
            for (r, n) in &by_ignore_reason {
                println!("  {r}: {n}");
            }
            println!("expectation mismatches: {}", result.expectation_mismatches.len());
            for m in &result.expectation_mismatches {
                println!(
                    "  [{}] {}:{}: {} — {}",
                    m.layer_name,
                    m.path.display(),
                    m.line,
                    m.type_name,
                    m.reason
                );
            }
            println!("boundary violations: {}", result.boundary_violations.len());
            for v in &result.boundary_violations {
                println!(
                    "  [{}] {}:{}: imports `{}` — {}",
                    v.boundary_name,
                    v.from_path.display(),
                    v.line,
                    v.imported_path,
                    v.reason
                );
            }
        }
        #[cfg(feature = "call-graph")]
        Commands::Callgraph { path, scip, precision, hub_threshold } => {
            use konpu::analyze::call_graph::{
                facts_from_project, facts_from_scip_file, CallGraph, Precision,
            };
            let prec = match precision.as_str() {
                "cha" => Precision::Cha,
                "rta" => Precision::Rta,
                other => {
                    eprintln!("konpu callgraph: unknown precision `{other}` (use cha|rta)");
                    std::process::exit(2);
                }
            };
            let facts = match &scip {
                Some(f) => facts_from_scip_file(std::path::Path::new(f)),
                None => facts_from_project(std::path::Path::new(&path)),
            };
            let mut facts = match facts {
                Ok(f) => f,
                Err(e) => {
                    eprintln!("konpu callgraph: {e}");
                    std::process::exit(1);
                }
            };
            // RTA: refine the instantiated set to actual construction sites found
            // in the source (tree-sitter), instead of SCIP's "any type reference"
            // which degenerates to CHA. See docs/layer2-call-graph-design.md §6.1.
            if prec == Precision::Rta {
                facts.instantiated =
                    konpu::analyze::call_graph::constructed_types(std::path::Path::new(&path));
            }
            let g = CallGraph::build(&facts, prec);
            let edges: usize = g.edges.iter().map(|s| s.len()).sum();
            let cycles = g.cycles();
            let hubs = g.hubs(hub_threshold);
            println!("== konpu callgraph ({precision}) ==");
            println!("functions: {}", facts.funcs.len());
            println!("call edges: {edges}");
            println!("cycles: {}", cycles.len());
            for scc in &cycles {
                let names: Vec<&str> = scc.iter().map(|&f| facts.funcs[f].name.as_str()).collect();
                println!("  cycle ({}): {}", scc.len(), names.join(" -> "));
            }
            println!("hubs (fan-in/out >= {hub_threshold}): {}", hubs.len());
            for &f in &hubs {
                println!(
                    "  {} (in={}, out={}) {}:{}",
                    facts.funcs[f].name,
                    g.in_degree(f),
                    g.out_degree(f),
                    facts.funcs[f].path.display(),
                    facts.funcs[f].line
                );
            }
        }
    }
}
