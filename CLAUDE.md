# konpu

## セットアップ

```bash
cargo build
cargo test
```

## コマンド実行

```bash
cargo build
cargo test
cargo test --test <name>
cargo run -- check <path>
```

## アーキテクチャ

```
Alloy (konpu.als) → oxidtr generate → Domain types (src/domain/)
                                            ↓
Rust source → proc macro → Annotations → Static analyzer → Diagnostics
                                            ↓
                                      konpu.toml → Template checker
```

### モジュール構成

```
models/
  konpu.als          Konpuドメインモデル (Alloy)
src/
  domain/            oxidtr生成の型定義
  annotation/        proc macro (#[konpu::monoid] etc.)
  analyze/           静的解析 (tree-sitter-rust)
  template/          konpu.toml テンプレート設定
  diagnostic/        診断出力
  main.rs            CLI エントリポイント
  lib.rs             ライブラリルート
```

## 技術スタック

- Rust (実装言語)
- 依存: clap 4 (CLI), syn/quote/proc-macro2 (proc macro), tree-sitter + tree-sitter-rust (静的解析)
- oxidtr: Alloyモデルからドメイン型生成
- 外部依存最小

## 設計原則

- **リンターであり、テストランナーではない** — 静的検査 + テスト存在・通過の確認のみ
- **proc macroでアノテーション** — コンパイル時に型情報にアクセス、構造的違反を検出
- **テンプレートで層別制約** — konpu.tomlでアーキテクチャ意図を宣言
- **oxidtrと疎結合** — アノテーションフォーマットのみが契約

## 開発ワークフロー

- main直push方式
- CIパス必須: `cargo test` + `cargo clippy`
- zero warnings ポリシー

### TDD

1. **Red**: 失敗するテストを先に書く
2. **Green**: テストを通す最小限のコードを実装する
3. **Refactor**: テストがグリーンの状態でコードを整理する

### コミット規約

- 各段階でテスト全パス + warning ゼロを確認してからコミット
- コミットメッセージ末尾に `Co-Authored-By: Claude Opus 4.6 (1M context) <noreply@anthropic.com>`
