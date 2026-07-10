//! `konpu.toml` テンプレート設定のパースと解釈。
//!
//! 設定ファイル例（roadmap §3.5）:
//! ```toml
//! [defaults]
//! max_propagation = 8
//!
//! [layers.domain]
//! path = "src/domain/**"
//! expect = ["monoid", "group"]
//! max_propagation = 4
//! ```

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::domain::konpu::AlgebraicStructure;
use crate::domain::konpu::HigherKindedStructure;

/// 層の expectation に対する違反 (Info 相当)。`analyze_full` で収集。
#[derive(Debug, Clone)]
pub struct LayerExpectationMismatch {
    pub layer_name: String,
    pub path: PathBuf,
    pub line: usize,
    pub type_name: String,
    pub reason: String,
}

/// 層間境界 1 件分（roadmap §3.5 `[boundaries.<name>]`）。
/// `from` 内ファイルから `to` 内ファイルへの参照 (use) は許可、
/// 逆方向 (`to` 内から `from` への参照) は違反。
/// `preserve` リストは保持されるべき代数的構造を宣言
/// （Phase 2-A 範囲では警告のみ、法則の保存そのものの検査は Phase 2-A 拡張）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Boundary {
    pub from: Option<String>,
    pub to: Option<String>,
    #[serde(default)]
    pub preserve: Vec<String>,
    /// preserve 検査の深刻度: "off" | "warn" | "error"（既定 warn）。
    #[serde(default)]
    pub preserve_severity: Option<String>,
    /// 有効化する検出器: "aggregate"（B）| "construct"（C）（既定=両方）。
    #[serde(default)]
    pub preserve_checks: Option<Vec<String>>,
}

/// preserve 検査の設定深刻度。law_test の有無で実効深刻度は下がりうる。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreserveSeverity {
    Off,
    Warn,
    Error,
}

impl PreserveSeverity {
    fn parse(s: Option<&str>) -> Self {
        match s {
            Some("off") => Self::Off,
            Some("error") => Self::Error,
            _ => Self::Warn, // 決定不能領域なので既定は warn
        }
    }
}

/// どの preserve 検出器を有効にするか。既定は両方（C は強化版=手書きマージ検出）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreserveChecks {
    pub aggregate: bool,
    pub construct: bool,
}

impl PreserveChecks {
    fn parse(v: Option<&[String]>) -> Self {
        match v {
            None => Self { aggregate: true, construct: true },
            Some(list) => Self {
                aggregate: list.iter().any(|s| s == "aggregate"),
                construct: list.iter().any(|s| s == "construct"),
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct ResolvedBoundary {
    pub name: String,
    pub from_pattern: String,
    pub to_pattern: String,
    pub preserve: Vec<AlgebraicStructure>,
    pub preserve_severity: PreserveSeverity,
    pub preserve_checks: PreserveChecks,
}

/// 層間境界の違反 (Phase 2-A: 逆方向参照のみ)。
#[derive(Debug, Clone)]
pub struct BoundaryViolation {
    pub boundary_name: String,
    pub from_path: PathBuf,
    pub to_path: PathBuf,
    pub line: usize,
    pub imported_path: String,
    pub reason: String,
}

/// `konpu.toml` のトップレベル。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Config {
    pub preset: Option<String>,
    #[serde(default)]
    pub defaults: Defaults,
    #[serde(default, rename = "layers")]
    pub layers: toml::Table,
    #[serde(default, rename = "boundaries")]
    pub boundaries: toml::Table,
    /// 解析対象から除外する glob パターン（root 相対）。例: `["tests/**", "**/fixtures/**"]`。
    #[serde(default)]
    pub exclude: Vec<String>,
    #[serde(default)]
    pub callgraph: Callgraph,
    /// アノテーション無しでも impl から代数構造を推論する。CLI `--infer` でも有効化。
    #[serde(default)]
    pub infer: bool,
}

/// `[callgraph]` セクション。`konpu callgraph` の閾値調整。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Callgraph {
    /// ハブ（fan-out/fan-in）とみなす次数の下限。未設定なら CLI 既定（8）。
    #[serde(default)]
    pub hub_threshold: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct Defaults {
    #[serde(default)]
    pub max_propagation: Option<i64>,
}

/// 1層の期待構造と伝播度上限。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct Layer {
    pub path: Option<String>,
    #[serde(default)]
    pub expect: Vec<String>,
    #[serde(default)]
    pub max_propagation: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ResolvedLayer {
    pub name: String,
    pub path_pattern: String,
    pub expect_structures: Vec<AlgebraicStructure>,
    pub expect_higher: Vec<HigherKindedStructure>,
    pub max_propagation: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub defaults_max: Option<i64>,
    pub layers: Vec<ResolvedLayer>,
    pub boundaries: Vec<ResolvedBoundary>,
    /// 解析対象から除外する glob（root 相対）。
    pub exclude: Vec<String>,
    /// `konpu callgraph` のハブ次数下限（未設定なら CLI 既定）。
    pub callgraph_hub_threshold: Option<usize>,
    /// アノテーション無し推論を有効にするか。
    pub infer: bool,
}

impl ResolvedConfig {
    pub fn empty() -> Self {
        Self {
            defaults_max: None,
            layers: Vec::new(),
            boundaries: Vec::new(),
            exclude: Vec::new(),
            callgraph_hub_threshold: None,
            infer: false,
        }
    }

    /// ファイル（root 相対に正規化）が exclude パターンのいずれかに一致するか。
    pub fn is_excluded(&self, file: &Path, root: &Path) -> bool {
        if self.exclude.is_empty() {
            return false;
        }
        let rel = rel_to_root(file, root);
        self.exclude.iter().any(|p| glob_match(p, &rel))
    }
}

/// ファイルパスを解析ルート起点の相対文字列にする。glob 照合の基準を CWD ではなく
/// 解析ルートに揃えるための共通ヘルパ（外部プロジェクトを別 CWD から解析しても合う）。
pub fn rel_to_root(file_path: &Path, root: &Path) -> String {
    file_path
        .strip_prefix(root)
        .unwrap_or(file_path)
        .to_string_lossy()
        .trim_start_matches("./")
        .to_string()
}

/// `konpu.toml` を読み込んで解決済み設定を返す。ファイル不在なら空設定。
pub fn load(path: &Path) -> ResolvedConfig {
    let Ok(text) = std::fs::read_to_string(path) else {
        return ResolvedConfig::empty();
    };
    parse(&text)
}

pub fn parse(text: &str) -> ResolvedConfig {
    let config: Config = toml::from_str(text).unwrap_or_default();
    let mut preset_layers = preset_layers(config.preset.as_deref());
    let user_layers = config
        .layers
        .iter()
        .filter_map(|(name, v)| {
            let layer: Layer = v.clone().try_into().ok()?;
            Some((name.clone(), layer))
        })
        .collect::<Vec<_>>();
    let preset_by_name: std::collections::HashMap<String, usize> = preset_layers
        .iter()
        .enumerate()
        .map(|(i, l)| (l.name.clone(), i))
        .collect();
    for (name, user_layer) in user_layers {
        if let Some(&idx) = preset_by_name.get(&name) {
            let p = &mut preset_layers[idx];
            if let Some(pp) = user_layer.path {
                p.path_pattern = pp;
            }
            if !user_layer.expect.is_empty() {
                p.expect_structures = parse_structures(&user_layer.expect);
                p.expect_higher = parse_higher(&user_layer.expect);
            }
            if let Some(mp) = user_layer.max_propagation {
                p.max_propagation = Some(mp);
            }
        } else {
            preset_layers.push(ResolvedLayer {
                name,
                path_pattern: user_layer.path.unwrap_or_default(),
                expect_structures: parse_structures(&user_layer.expect),
                expect_higher: parse_higher(&user_layer.expect),
                max_propagation: user_layer.max_propagation,
            });
        }
    }
    let mut boundaries = Vec::new();
    for (name, v) in config.boundaries {
        let b: Boundary = match v.try_into() {
            Ok(b) => b,
            Err(_) => continue,
        };
        let from = match b.from {
            Some(f) => f,
            None => continue,
        };
        let to = match b.to {
            Some(t) => t,
            None => continue,
        };
        boundaries.push(ResolvedBoundary {
            name,
            from_pattern: from,
            to_pattern: to,
            preserve: parse_structures(&b.preserve),
            preserve_severity: PreserveSeverity::parse(b.preserve_severity.as_deref()),
            preserve_checks: PreserveChecks::parse(b.preserve_checks.as_deref()),
        });
    }
    ResolvedConfig {
        defaults_max: config.defaults.max_propagation,
        layers: preset_layers,
        boundaries,
        exclude: config.exclude,
        callgraph_hub_threshold: config.callgraph.hub_threshold,
        infer: config.infer,
    }
}

fn parse_structures(expect: &[String]) -> Vec<AlgebraicStructure> {
    expect
        .iter()
        .filter_map(|s| match s.as_str() {
            "magma" => Some(AlgebraicStructure::Magma),
            "semigroup" => Some(AlgebraicStructure::Semigroup),
            "monoid" => Some(AlgebraicStructure::Monoid),
            "group" => Some(AlgebraicStructure::Group),
            _ => None,
        })
        .collect()
}

fn parse_higher(expect: &[String]) -> Vec<HigherKindedStructure> {
    expect
        .iter()
        .filter_map(|s| match s.as_str() {
            "functor" => Some(HigherKindedStructure::Functor),
            "applicative" => Some(HigherKindedStructure::Applicative),
            "monad" => Some(HigherKindedStructure::MonadS),
            _ => None,
        })
        .collect()
}

/// glob パターン（`**` 含む）をパスにマッチさせる。簡易版:
/// `**` は任意の0以上のディレクトリ、`*` は1コンポーネント内の任意文字列。
pub fn glob_match(pattern: &str, path: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').collect();
    let pth: Vec<&str> = path.split('/').collect();
    glob_match_i(&pat, 0, &pth, 0)
}

fn glob_match_i(pat: &[&str], pi: usize, pth: &[&str], ti: usize) -> bool {
    if pi == pat.len() {
        return ti == pth.len();
    }
    if pat[pi] == "**" {
        if pi + 1 == pat.len() {
            return true;
        }
        for k in ti..=pth.len() {
            if glob_match_i(pat, pi + 1, pth, k) {
                return true;
            }
        }
        return false;
    }
    if ti == pth.len() {
        return false;
    }
    if comp_match(pat[pi], pth[ti]) && glob_match_i(pat, pi + 1, pth, ti + 1) {
        return true;
    }
    false
}

fn comp_match(pat: &str, comp: &str) -> bool {
    if pat == "*" {
        return true;
    }
    if !pat.contains('*') {
        return pat == comp;
    }
    let pat_bytes = pat.as_bytes();
    let comp_bytes = comp.as_bytes();
    let m = pat_bytes.len();
    let n = comp_bytes.len();
    let mut dp = vec![vec![false; n + 1]; m + 1];
    dp[0][0] = true;
    for i in 1..=m {
        if pat_bytes[i - 1] == b'*' {
            dp[i][0] = dp[i - 1][0];
        }
    }
    for i in 1..=m {
        for j in 1..=n {
            if pat_bytes[i - 1] == b'*' {
                dp[i][j] = dp[i - 1][j] || dp[i][j - 1];
            } else if pat_bytes[i - 1] == comp_bytes[j - 1] {
                dp[i][j] = dp[i - 1][j - 1];
            }
        }
    }
    dp[m][n]
}

/// 指定ファイルパス（解析ルート `root` 起点で相対化）にマッチする最初の層を返す。
pub fn match_layer<'a>(
    config: &'a ResolvedConfig,
    file_path: &Path,
    root: &Path,
) -> Option<&'a ResolvedLayer> {
    let s = rel_to_root(file_path, root);
    config.layers.iter().find(|l| {
        !l.path_pattern.is_empty() && glob_match(&l.path_pattern, &s)
    })
}

/// その層またはデフォルトの `max_propagation` を返す。
pub fn threshold(config: &ResolvedConfig, layer: Option<&ResolvedLayer>) -> Option<i64> {
    layer
        .and_then(|l| l.max_propagation)
        .or(config.defaults_max)
}

fn preset_layers(preset: Option<&str>) -> Vec<ResolvedLayer> {
    match preset {
        Some("ddd") => vec![
            ResolvedLayer {
                name: "domain".to_string(),
                path_pattern: "src/domain/**".to_string(),
                expect_structures: vec![
                    AlgebraicStructure::Monoid,
                    AlgebraicStructure::Group,
                ],
                expect_higher: Vec::new(),
                max_propagation: Some(4),
            },
            ResolvedLayer {
                name: "application".to_string(),
                path_pattern: "src/application/**".to_string(),
                expect_structures: Vec::new(),
                expect_higher: vec![HigherKindedStructure::MonadS],
                max_propagation: Some(8),
            },
            ResolvedLayer {
                name: "infra".to_string(),
                path_pattern: "src/infra/**".to_string(),
                expect_structures: Vec::new(),
                expect_higher: vec![HigherKindedStructure::Functor],
                max_propagation: Some(-1),
            },
        ],
        Some("hexagonal") => Vec::new(),
        Some("clean") => Vec::new(),
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_when_file_absent() {
        let cfg = load(Path::new("/nonexistent/konpu.toml"));
        assert!(cfg.layers.is_empty());
        assert!(cfg.defaults_max.is_none());
    }

    #[test]
    fn parses_defaults_only() {
        let cfg = parse("[defaults]\nmax_propagation = 8\n");
        assert_eq!(cfg.defaults_max, Some(8));
        assert!(cfg.layers.is_empty());
    }

    #[test]
    fn parses_layer() {
        let cfg = parse(
            "[defaults]\nmax_propagation = 8\n\
             [layers.domain]\npath = \"src/domain/**\"\n\
             expect = [\"monoid\"]\nmax_propagation = 4\n",
        );
        assert_eq!(cfg.layers.len(), 1);
        let l = &cfg.layers[0];
        assert_eq!(l.name, "domain");
        assert_eq!(l.path_pattern, "src/domain/**");
        assert_eq!(l.max_propagation, Some(4));
        assert!(l.expect_structures.contains(&AlgebraicStructure::Monoid));
    }

    #[test]
    fn preset_ddd_provides_three_layers() {
        let cfg = parse("preset = \"ddd\"\n");
        assert_eq!(cfg.layers.len(), 3);
        let names: Vec<&str> = cfg.layers.iter().map(|l| l.name.as_str()).collect();
        assert_eq!(names, vec!["domain", "application", "infra"]);
    }

    #[test]
    fn user_layer_overrides_preset_by_name() {
        let cfg = parse(
            "preset = \"ddd\"\n\
             [layers.domain]\nmax_propagation = 6\n",
        );
        let dom = cfg.layers.iter().find(|l| l.name == "domain").unwrap();
        assert_eq!(dom.max_propagation, Some(6));
    }

    #[test]
    fn threshold_uses_layer_then_default() {
        let mut cfg = ResolvedConfig::empty();
        cfg.defaults_max = Some(8);
        let layer = ResolvedLayer {
            name: "x".to_string(),
            path_pattern: "x/**".to_string(),
            expect_structures: Vec::new(),
            expect_higher: Vec::new(),
            max_propagation: Some(2),
        };
        assert_eq!(threshold(&cfg, Some(&layer)), Some(2));
        assert_eq!(threshold(&cfg, None), Some(8));
    }

    #[test]
    fn glob_match_double_star() {
        assert!(glob_match("src/domain/**", "src/domain/foo.rs"));
        assert!(glob_match("src/domain/**", "src/domain/foo/bar.rs"));
        assert!(glob_match("src/domain/**", "src/domain"));
        assert!(!glob_match("src/domain/**", "src/infra/foo.rs"));
    }

    #[test]
    fn glob_match_single_star() {
        assert!(glob_match("src/*.rs", "src/foo.rs"));
        assert!(!glob_match("src/*.rs", "src/foo/bar.rs"));
    }

    #[test]
    fn callgraph_hub_threshold_parsed() {
        let cfg = parse("[callgraph]\nhub_threshold = 5\n");
        assert_eq!(cfg.callgraph_hub_threshold, Some(5));
        let empty = parse("");
        assert_eq!(empty.callgraph_hub_threshold, None);
    }

    #[test]
    fn exclude_globs_parsed_and_matched() {
        let cfg = parse("exclude = [\"tests/**\", \"**/fixtures/**\"]\n");
        assert_eq!(cfg.exclude.len(), 2);
        let root = Path::new("/proj");
        assert!(cfg.is_excluded(Path::new("/proj/tests/it.rs"), root));
        assert!(cfg.is_excluded(Path::new("/proj/src/analyze/fixtures/x.rs"), root));
        assert!(!cfg.is_excluded(Path::new("/proj/src/analyze/mod.rs"), root));
    }

    #[test]
    fn boundary_preserve_defaults() {
        let cfg = parse(
            "[boundaries.d2i]\nfrom = \"src/domain/**\"\nto = \"src/infra/**\"\n\
             preserve = [\"monoid\"]\n",
        );
        assert_eq!(cfg.boundaries.len(), 1);
        let b = &cfg.boundaries[0];
        assert_eq!(b.preserve_severity, PreserveSeverity::Warn);
        // default: both detectors on
        assert!(b.preserve_checks.aggregate);
        assert!(b.preserve_checks.construct);
    }

    #[test]
    fn boundary_preserve_explicit_severity_and_checks() {
        let cfg = parse(
            "[boundaries.d2i]\nfrom = \"src/domain/**\"\nto = \"src/infra/**\"\n\
             preserve = [\"monoid\"]\npreserve_severity = \"error\"\n\
             preserve_checks = [\"aggregate\"]\n",
        );
        let b = &cfg.boundaries[0];
        assert_eq!(b.preserve_severity, PreserveSeverity::Error);
        assert!(b.preserve_checks.aggregate);
        assert!(!b.preserve_checks.construct); // opted out of construct
    }

    #[test]
    fn match_layer_finds_first() {
        let cfg = parse(
            "[layers.domain]\npath = \"src/domain/**\"\nmax_propagation = 4\n\
             [layers.infra]\npath = \"src/infra/**\"\nmax_propagation = -1\n",
        );
        let root = Path::new("");
        let l = match_layer(&cfg, Path::new("src/domain/konpu.rs"), root);
        assert!(l.is_some());
        assert_eq!(l.unwrap().name, "domain");
        let l = match_layer(&cfg, Path::new("src/infra/db.rs"), root);
        assert!(l.is_some());
        assert_eq!(l.unwrap().name, "infra");
        let l = match_layer(&cfg, Path::new("src/lib.rs"), root);
        assert!(l.is_none());
    }
}