//! `// konpu:` コメント注釈の言語非依存パーサ。
//!
//! Swift / Kotlin など tree-sitter で読む言語は、未知属性をコンパイラが弾くため
//! アノテーションをコメントで書く（推論優先 + 明示したい時だけコメント）。書式は
//! 全言語共通の契約: `// konpu: <head>(<key: value>, <positional>, ...)`。
//! この本文パースだけを共有し、コメントノードの走査と「直後の宣言」の解決は各言語の
//! 抽出器が担う（ノードの種別・名前取得が言語ごとに違うため）。

use std::collections::HashMap;

use crate::domain::konpu::{AlgebraicStructure, HigherKindedStructure};

/// `// konpu: head(args)` を解析した結果。
pub struct Directive {
    pub head: String,
    pub positional: Vec<String>,
    pub kwargs: HashMap<String, String>,
}

/// コメントテキストから konpu ディレクティブを解析。`konpu:` 以外は None。
/// 先頭の `//` はどの言語でも同じなので剥がす。
pub fn parse_directive(comment_text: &str) -> Option<Directive> {
    let t = comment_text.trim_start_matches('/').trim();
    let rest = t.strip_prefix("konpu:")?.trim();
    let (head, argstr) = match rest.find('(') {
        Some(i) => {
            let end = rest.rfind(')').unwrap_or(rest.len());
            (rest[..i].trim(), &rest[i + 1..end])
        }
        None => (rest, ""),
    };
    let mut positional = Vec::new();
    let mut kwargs = HashMap::new();
    for part in argstr.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        if let Some((k, v)) = part.split_once(':') {
            kwargs.insert(k.trim().to_string(), v.trim().trim_matches('"').to_string());
        } else {
            positional.push(part.to_string());
        }
    }
    Some(Directive { head: head.to_string(), positional, kwargs })
}

/// 代数構造ディレクティブ名 → 構造。
pub fn structure_from(head: &str) -> Option<AlgebraicStructure> {
    match head {
        "monoid" => Some(AlgebraicStructure::Monoid),
        "group" => Some(AlgebraicStructure::Group),
        "semigroup" => Some(AlgebraicStructure::Semigroup),
        "magma" => Some(AlgebraicStructure::Magma),
        _ => None,
    }
}

/// `higher: functor|applicative|monad` → 高階構造。
pub fn higher_from(value: &str) -> Option<HigherKindedStructure> {
    match value {
        "functor" => Some(HigherKindedStructure::Functor),
        "applicative" => Some(HigherKindedStructure::Applicative),
        "monad" => Some(HigherKindedStructure::MonadS),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_kwargs_and_positional() {
        let d = parse_directive("// konpu: monoid(op: combine, identity: zero, higher: functor)").unwrap();
        assert_eq!(d.head, "monoid");
        assert_eq!(d.kwargs.get("op").unwrap(), "combine");
        assert_eq!(d.kwargs.get("higher").unwrap(), "functor");
        let d2 = parse_directive("// konpu: law(associativity, left_identity)").unwrap();
        assert_eq!(d2.positional, vec!["associativity", "left_identity"]);
    }

    #[test]
    fn non_konpu_comment_is_none() {
        assert!(parse_directive("// just a comment").is_none());
    }

    #[test]
    fn strips_quotes_in_values() {
        let d = parse_directive("// konpu: ignore(reason: intentional, note: \"order matters\")").unwrap();
        assert_eq!(d.kwargs.get("note").unwrap(), "order matters");
    }
}
