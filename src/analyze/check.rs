use crate::domain::konpu::{
    AlgebraicDeclaration, Diagnostic, DiagnosticRule, OperationName, Severity,
};

use super::extract::{AnalyzedDeclaration, ImplInfo, SelfKind};

pub fn check_declaration(
    decl: &AnalyzedDeclaration,
    impls: &[ImplInfo],
) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    let declaration = AlgebraicDeclaration {
        targetStructure: decl.target_structure.clone(),
        higherKinded: decl.higher_kinded.clone(),
        operationName: OperationName,
        identityName: None,
        inverseName: None,
    };
    let matching: Vec<&ImplInfo> = impls
        .iter()
        .filter(|i| i.type_name == decl.type_name)
        .collect();
    let op_method = matching
        .iter()
        .flat_map(|i| i.methods.iter())
        .find(|m| m.name == decl.operation_name);

    let mut had_error = false;

    if decl.target_structure.rank() >= 2 {
        let id_name = decl.identity_name.as_deref();
        if id_name.is_none() || !has_method(&matching, id_name.unwrap_or("")) {
            had_error = true;
            out.push(Diagnostic {
                severity: Severity::Error,
                declaration: declaration.clone(),
                rule: DiagnosticRule::MissingIdentity,
            });
        }
    }

    if decl.target_structure.rank() >= 3 {
        let inv_name = decl.inverse_name.as_deref();
        if inv_name.is_none() || !has_method(&matching, inv_name.unwrap_or("")) {
            had_error = true;
            out.push(Diagnostic {
                severity: Severity::Error,
                declaration: declaration.clone(),
                rule: DiagnosticRule::MissingInverse,
            });
        }
    }

    if let Some(op) = op_method {
        let mut violated = false;
        if op.self_param == Some(SelfKind::MutRef) {
            violated = true;
        }
        match &op.return_type {
            None => violated = true,
            Some(t) => {
                let t = t.trim();
                if t == "()" || t.is_empty() {
                    violated = true;
                }
            }
        }
        if op.is_assoc_fn && op.params.len() != 2 {
            violated = true;
        }
        if violated {
            had_error = true;
            out.push(Diagnostic {
                severity: Severity::Error,
                declaration: declaration.clone(),
                rule: DiagnosticRule::ClosureViolation,
            });
        } else if !had_error && op_returns_self(decl, op) {
            out.push(Diagnostic {
                severity: Severity::Info,
                declaration: declaration.clone(),
                rule: DiagnosticRule::AssociativityConfidence,
            });
        }
    }

    if decl.higher_kinded.is_some() {
        let map_method = matching
            .iter()
            .flat_map(|i| i.methods.iter())
            .find(|m| m.name == "map");
        if let Some(map) = map_method {
            let map_violation = map.self_param == Some(SelfKind::MutRef)
                || map
                    .return_type
                    .as_deref()
                    .is_none_or(|t| t.trim() == "()" || t.trim().is_empty())
                || (!map.is_assoc_fn && map.params.len() != 1)
                || (map.is_assoc_fn && map.params.len() != 2);
            if map_violation {
                had_error = true;
                out.push(Diagnostic {
                    severity: Severity::Error,
                    declaration: declaration.clone(),
                    rule: DiagnosticRule::MapSignatureViolation,
                });
            }
        }
    }

    let _ = had_error;
    out
}

fn has_method(impls: &[&ImplInfo], name: &str) -> bool {
    impls
        .iter()
        .flat_map(|i| i.methods.iter())
        .any(|m| m.name == name)
}

fn op_returns_self(decl: &AnalyzedDeclaration, op: &super::extract::MethodInfo) -> bool {
    let ret = match &op.return_type {
        Some(t) => t.trim().to_string(),
        None => return false,
    };
    let self_ret = ret == "Self" || ret == decl.type_name || ret.contains("Self");
    let self_param_ok = matches!(op.self_param, Some(SelfKind::Ref) | Some(SelfKind::Owned));
    if op.is_assoc_fn {
        if op.params.len() != 2 {
            return false;
        }
        if !self_ret {
            return false;
        }
        let all_self = op.params.iter().all(|p| {
            let p = p.trim();
            p == decl.type_name || p == "Self" || p.contains(&format!("{} ", decl.type_name))
        });
        if !all_self {
            return false;
        }
        return true;
    }
    if !self_param_ok {
        return false;
    }
    if !self_ret {
        return false;
    }
    true
}