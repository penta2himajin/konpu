# Call Graph Provider — 設計方針

> ステータス：方針確定。実装は Phase 0（スタブ）まで完了、Phase 1（rust-analyzer 統合）以降は別セッション。
> 関連：`docs/roadmap.md` セクション5 (Phase 2-A)、`docs/layer2-call-graph-design.md`、`docs/layer3-decidability-limits.md`

## 1. 背景

Phase 2-A `[boundaries]` の `preserve` 検査は「ドメイン層のモノイド則がインフラ層を通過後も保存されること」を要求するが、これは跨層のデータフロー/呼び出し追跡を必要とする。静的テキストベースの近似（同名型の rank 降格検出）には限界があり、本格的なコールグラフ構築が必要。

`docs/layer2-call-graph-design.md` が示すとおり、完全な静的コールグラフ構築は決定不能（動的ディスパッチ・クロージャ・FFI）であり、健全な過大近似（偽陽性許容、偽陰性なし）に留める方針を採る。

## 2. 採用方針：別クレート `konpu-cg` + optional feature

### 2.1 依存構成

- `konpu-cg` は `konpu` と同じ workspace 配下の**ライブラリクレート**として新設
- `konpu` 本体は `konpu-cg` を `optional = true` 依存として宣言
- feature flag `call-graph = ["dep:konpu-cg"]` で有効化
- feature なしでも `konpu check` は従来通り動作（コールグラフが無ければ同名型 rank 降格の近似検査のみ）

### 2.2 インターフェース不変性

`konpu` 本体は `CallGraphProvider` trait を公開し、`analyze_full` がその trait object を受け取る。デフォルト dummy は空のコールグラフを返す。将来的に StableMIR ベースや rust-analyzer HIR ベースの実装を差し替えても、`konpu` 側 API は不変。LSP 根差の設計にすれば、Kotlin/TypeScript 対応時も Language Server アダプタへの差し替えが容易。

### 2.3 メリット

- `konpu` 本体は最小依存を保てる（コールグラフ機能なしでもビルド可）
- 責務分離：代数的検査（第3層）とコールグラフトポロジ検査（第2b層）は別軸
- テスト容易性：dummy provider で境界検査ロジックを独立テスト可

## 3. rust-analyzer HIR / call hierarchy を採用

### 3.1 理由

- **変化耐性**: StableMIR は安定化進行中で破壊的変更リスクあり。rust-analyzer は広く使われ安定
- **多言語対応**: Language Server Protocol (LSP) に根差するため、Kotlin/TypeScript 拡張時に各言語の LSP サーバに差し替えやすい
- **既存資産**: rust-analyzer は `call_hierarchy/incomingCalls` / `outgoingCalls` 等 LSP メソッドを既に提供。自前で vtable 解決を書かなくても最先端の trait impl 発見機能が再利用可能

### 3.2 通信構成

rust-analyzer を外部プロセスとして spawn し stdio JSON-RPC で通信する programmatic LSP client 実装とする。`konpu-cg` クレート内に LSP client wrapper を持ち、外部依存は最小限に抑える。

## 4. 段階的実装

### Phase 0 (本セッション完了) — スタブ + インターフェース確定

- `konpu-cg` クレート新設
- `konpu::analyze::CallGraphProvider` trait 定義 + dummy 実装 (空のコールグラフ)
- `analyze_full` が `Option<&dyn CallGraphProvider>` を受ける追加 API
- 既存 `analyze_full(path, &config)` は dummy provider で委譲
- `konpu-cg` の実体は空 (stub)。rust-analyzer 通信ロジックは未実装

### Phase 1 (別セッション) — rust-analyzer 統合

- `konpu-cg` に LSP client wrapper 実装
- rust-analyzer を spawn し、`textDocument/didOpen` → `call_hierarchy/incomingCalls` / `outgoingCalls` を呼び出し
- `CallGraphProvider` の実体として `RustAnalyzerCallGraph` を提供
- `konpu` CLI に `--call-graph` フラグ（feature `call-graph` 有効時のみ）追加

### Phase 2 以降 — RTA 拡張・多言語対応

- 健全な過大近似路線: CHA → RTA（実際にインスタンス化された型で候補を絞る）
- LSP アダプタ差し替えで Kotlin LSP / tsserver 等に対応

## 5. Konpu 本体 API 形

```rust
pub trait CallGraphProvider {
    fn resolve_outgoing_calls(
        &self,
        file_path: &Path,
        line: usize,
        column: usize,
    ) -> Vec<CallTarget>;
}

pub struct CallTarget {
    pub target_path: PathBuf,
    pub target_line: usize,
    pub target_name: String,
}

pub fn analyze_full_with_cg(
    path: &Path,
    config: &template::ResolvedConfig,
    provider: Option<&dyn CallGraphProvider>,
) -> AnalysisResult;
```

- `analyze_full(path, &config)` は引き続き `analyze_full_with_cg(path, config, None)` に委譲
- `preserve` 検査は provider が `Some` のときコールグラフ追跡で、`None` のときは従来の同名型 rank 降格近似にフォールバック

## 6. 既知の限界

- `konpu-cg` feature なしでも `konpu` は動作するが、その場合 preserve 検査は近似止まり
- rust-analyzer はターゲットクレートのビルドなしでもHIRからコール階層を取れるが、完全解決は動的ディスパッチの健全な過大近似まま。FFI・プラグイン・関数ポインタの格納→呼び出しパターンは検出不可
- これらは layer2-call-graph-design.md §6「消えない限界」と整合