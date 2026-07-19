//! ベースライン — 既存プロジェクトへの導入用。
//!
//! `konpu baseline` が現在の全違反を JSON ファイルに記録。
//! `konpu check` 時に同じパス＋同じ違反キーが baseline に登録されていれば
//! 「既知」として扱い、新規違反のみをレポートする。

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::AnalyzedDiagnostic;

fn severity_str(s: &crate::domain::konpu::Severity) -> &'static str {
    use crate::domain::konpu::Severity;
    match s {
        Severity::Error => "Error",
        Severity::Warning => "Warning",
        Severity::Info => "Info",
    }
}

fn rule_str(r: &crate::domain::konpu::DiagnosticRule) -> &'static str {
    use crate::domain::konpu::DiagnosticRule;
    match r {
        DiagnosticRule::MissingIdentity => "MissingIdentity",
        DiagnosticRule::MissingInverse => "MissingInverse",
        DiagnosticRule::ClosureViolation => "ClosureViolation",
        DiagnosticRule::MapSignatureViolation => "MapSignatureViolation",
        DiagnosticRule::MissingLawTest => "MissingLawTest",
        DiagnosticRule::FailingLawTest => "FailingLawTest",
        DiagnosticRule::PropagationExceeded => "PropagationExceeded",
        DiagnosticRule::AssociativityConfidence => "AssociativityConfidence",
        DiagnosticRule::KnownAssociativityRisk => "KnownAssociativityRisk",
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct BaselineEntry {
    pub path: String,
    pub line: usize,
    pub severity: String,
    pub rule: String,
    pub target: String,
}

impl BaselineEntry {
    pub fn from_diag(d: &AnalyzedDiagnostic) -> Self {
        Self {
            path: d.path.display().to_string(),
            line: d.line,
            severity: severity_str(&d.diag.severity).to_string(),
            rule: rule_str(&d.diag.rule).to_string(),
            target: format!("{:?}", d.diag.declaration.targetStructure),
        }
    }
}

pub fn load(path: &Path) -> std::collections::HashSet<BaselineEntry> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return std::collections::HashSet::new();
    };
    serde_json::from_str::<Vec<BaselineEntry>>(&text)
        .unwrap_or_default()
        .into_iter()
        .collect()
}

pub fn save(path: &Path, entries: &[BaselineEntry]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let text = serde_json::to_string_pretty(entries)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    std::fs::write(path, text)
}

/// 現在の diagnostics を baseline と比較し、baseline に登録されていない
/// 「新規」のみのリストを返す。
pub fn filter_new(
    diagnostics: Vec<AnalyzedDiagnostic>,
    baseline: &std::collections::HashSet<BaselineEntry>,
) -> Vec<AnalyzedDiagnostic> {
    diagnostics
        .into_iter()
        .filter(|d| !baseline.contains(&BaselineEntry::from_diag(d)))
        .collect()
}

pub fn entries_from(diagnostics: &[AnalyzedDiagnostic]) -> Vec<BaselineEntry> {
    diagnostics.iter().map(BaselineEntry::from_diag).collect()
}

pub fn default_path() -> PathBuf {
    PathBuf::from(".konpu/baseline.json")
}