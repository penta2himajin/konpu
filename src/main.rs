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
        /// Infer algebraic structures from impls even without annotations.
        #[arg(long)]
        infer: bool,
        /// Path to captured `cargo test` output. Law tests that appear in its
        /// `failures:` block are reported as FailingLawTest (Error).
        #[arg(long)]
        test_results: Option<String>,
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
    /// Print a summary: diagnostics, ignores, declarations, compliance gap
    Report {
        /// Path to analyze
        path: String,
        /// Path to konpu.toml (default: ./konpu.toml if present)
        #[arg(long)]
        config: Option<String>,
        /// Path to captured `cargo test` output. Refines the compliance gap by
        /// splitting covered laws into passing vs failing (else all covered = passing).
        #[arg(long)]
        test_results: Option<String>,
        /// Infer algebraic structures from impls even without annotations.
        #[arg(long)]
        infer: bool,
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
        /// Report functions whose fan-in or fan-out is at least this. Overrides
        /// [callgraph] hub_threshold in konpu.toml; both fall back to 8.
        #[arg(long)]
        hub_threshold: Option<usize>,
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

/// 設定ファイルのパスを決める。`--config` 明示が最優先。無ければ、ディレクトリ解析
/// では `<path>/konpu.toml` を優先し（外部プロジェクトをそのまま解析できる）、
/// 無ければ CWD の `konpu.toml` にフォールバックする。
fn resolve_config_path(explicit: Option<&str>, analyze_path: &std::path::Path) -> std::path::PathBuf {
    if let Some(c) = explicit {
        return std::path::PathBuf::from(c);
    }
    let in_dir = analyze_path.join("konpu.toml");
    if analyze_path.is_dir() && in_dir.is_file() {
        return in_dir;
    }
    std::path::PathBuf::from("konpu.toml")
}

/// `.swift` のみで `.rs` を含まないディレクトリを Swift プロジェクトと判定。
#[cfg(feature = "call-graph")]
fn is_swift_project(path: &std::path::Path) -> bool {
    use konpu::analyze::parser::{collect_source_files, Language};
    let files = collect_source_files(path);
    let swift = files.iter().filter(|(_, l)| *l == Language::Swift).count();
    let rust = files.iter().filter(|(_, l)| *l == Language::Rust).count();
    swift > 0 && rust == 0
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Commands::Check { path, config, baseline, call_graph, infer, test_results } => {
            use konpu::analyze::baseline;
            use konpu::analyze::{check, template};
            use konpu::domain::konpu::Severity;
            let p = std::path::PathBuf::from(path);
            let config_path = resolve_config_path(config.as_deref(), &p);
            let mut resolved = template::load(&config_path);
            resolved.infer = resolved.infer || infer;
            let failed_tests = match test_results {
                Some(rp) => match std::fs::read_to_string(&rp) {
                    Ok(out) => check::parse_failed_tests(&out),
                    Err(e) => {
                        eprintln!("konpu check: failed to read --test-results {rp}: {e}");
                        std::process::exit(2);
                    }
                },
                None => std::collections::HashSet::new(),
            };
            let diagnostics = konpu::analyze::analyze_with_results(&p, &resolved, &failed_tests).diagnostics;
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
            let config_path = resolve_config_path(config.as_deref(), &p);
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
            let config_path = resolve_config_path(config.as_deref(), &p);
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
        Commands::Report { path, config, test_results, infer } => {
            use konpu::analyze::{check, template};
            use konpu::domain::konpu::Severity;
            use std::collections::BTreeMap;
            let p = std::path::PathBuf::from(path);
            let config_path = resolve_config_path(config.as_deref(), &p);
            let mut resolved = template::load(&config_path);
            resolved.infer = resolved.infer || infer;
            let has_results = test_results.is_some();
            let failed_tests = match test_results {
                Some(rp) => match std::fs::read_to_string(&rp) {
                    Ok(out) => check::parse_failed_tests(&out),
                    Err(e) => {
                        eprintln!("konpu report: failed to read --test-results {rp}: {e}");
                        std::process::exit(2);
                    }
                },
                None => std::collections::HashSet::new(),
            };
            let result = konpu::analyze::analyze_with_results(&p, &resolved, &failed_tests);
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
            // 充足ギャップ（軸2）: 各宣言の required 法則のうち passing の割合。
            let compliance = check::law_compliance(&result.declarations, &result.law_tests, &failed_tests);
            let (req_total, pass_total): (usize, usize) =
                compliance.iter().fold((0, 0), |(r, p), c| (r + c.required, p + c.passing));
            let overall_gap = if req_total == 0 { 0.0 } else { 1.0 - pass_total as f64 / req_total as f64 };
            println!(
                "compliance gap: {:.2} ({}/{} laws verified across {} structure(s)){}",
                overall_gap,
                pass_total,
                req_total,
                compliance.len(),
                if has_results { "" } else { " [presence only; pass --test-results to detect failing]" }
            );
            for c in &compliance {
                println!(
                    "  {} ({:?}): gap {:.2} — {}/{} verified [pass {}, fail {}, missing {}]",
                    c.type_name,
                    c.structure,
                    c.gap(),
                    c.passing,
                    c.required,
                    c.passing,
                    c.failing,
                    c.missing
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
            // Swift プロジェクト（.swift のみ）は tree-sitter で Facts を直接構築
            // （外部ツール不要、instantiated も構築サイトで埋まる）。それ以外は
            // rust-analyzer/SCIP 経路。--scip 指定時は常に SCIP。
            let is_swift = scip.is_none() && is_swift_project(std::path::Path::new(&path));
            let facts = match &scip {
                Some(f) => facts_from_scip_file(std::path::Path::new(f)),
                None if is_swift => {
                    Ok(konpu::analyze::call_graph_swift::facts_from_swift_project(std::path::Path::new(&path)))
                }
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
            // Swift facts already carry construction sites, so skip the Rust refiner.
            if prec == Precision::Rta && !is_swift {
                facts.instantiated =
                    konpu::analyze::call_graph::constructed_types(std::path::Path::new(&path));
            }
            // ハブ閾値: CLI flag > konpu.toml [callgraph].hub_threshold > 既定 8。
            let cfg = konpu::analyze::template::load(&std::path::Path::new(&path).join("konpu.toml"));
            let hub_threshold = hub_threshold.or(cfg.callgraph_hub_threshold).unwrap_or(8);
            let g = CallGraph::build(&facts, prec);
            let edges: usize = g.edges.iter().map(|s| s.len()).sum();
            use konpu::analyze::call_graph::cycle_is_cross_module;
            let cycles = g.cycles();
            // 循環を3種に分ける（actionable なものだけ前面に）:
            //  - cross-module cycle: 複数ファイルに跨る真の依存もつれ（要対応）
            //  - intra-module recursion: 単一ファイル内の相互再帰（再帰下降パーサ等、良性）
            //  - self-recursion: 自分を呼ぶだけ（良性）
            let (mutual, self_rec): (Vec<_>, Vec<_>) =
                cycles.into_iter().partition(|scc| scc.len() > 1);
            let (cross, intra): (Vec<_>, Vec<_>) = mutual
                .into_iter()
                .partition(|scc| cycle_is_cross_module(scc, &facts));
            println!("== konpu callgraph ({precision}) ==");
            println!("functions: {}", facts.funcs.len());
            println!("call edges: {edges}");
            println!("cross-module cycles (circular dependencies): {}", cross.len());
            for scc in &cross {
                let names: Vec<&str> = scc.iter().map(|&f| facts.funcs[f].name.as_str()).collect();
                println!("  cycle ({}): {}", scc.len(), names.join(" -> "));
            }
            println!("intra-module recursion (benign): {}", intra.len());
            for scc in &intra {
                let f = scc[0];
                let names: Vec<&str> = scc.iter().map(|&f| facts.funcs[f].name.as_str()).collect();
                println!(
                    "  {} ({} fns) {}",
                    facts.funcs[f].path.display(),
                    scc.len(),
                    names.first().copied().unwrap_or_default()
                );
            }
            println!("self-recursion (benign): {}", self_rec.len());
            for scc in &self_rec {
                let f = scc[0];
                println!(
                    "  {} {}:{}",
                    facts.funcs[f].name,
                    facts.funcs[f].path.display(),
                    facts.funcs[f].line
                );
            }
            let print_hub = |f: konpu::analyze::call_graph::FuncId| {
                println!(
                    "  {} (out={}, in={}) {}:{}",
                    facts.funcs[f].name,
                    g.out_degree(f),
                    g.in_degree(f),
                    facts.funcs[f].path.display(),
                    facts.funcs[f].line
                );
            };
            // fan-out: 神関数の匂い（多くを呼ぶ）→ 分解候補。
            let mut fan_out = g.fan_out_hubs(hub_threshold);
            fan_out.sort_by_key(|&f| std::cmp::Reverse(g.out_degree(f)));
            println!(
                "fan-out hubs (calls >= {hub_threshold} — decomposition candidates): {}",
                fan_out.len()
            );
            for f in fan_out {
                print_hub(f);
            }
            // fan-in: 広く使われるヘルパー（多くから呼ばれる）→ 大抵健全、変更の集中点。
            let mut fan_in = g.fan_in_hubs(hub_threshold);
            fan_in.sort_by_key(|&f| std::cmp::Reverse(g.in_degree(f)));
            println!(
                "fan-in hubs (called >= {hub_threshold} — shared helpers / change chokepoints): {}",
                fan_in.len()
            );
            for f in fan_in {
                print_hub(f);
            }
        }
    }
}
