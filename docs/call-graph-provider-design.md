# Call Graph Provider — 設計方針

> ステータス：Phase 0（スタブ）・Phase 1（rust-analyzer/SCIP 統合）・Phase 2（CHA→RTA）実装済み。
> `call-graph` feature 有効時、`konpu callgraph <path>` で循環・ハブを検出できる（Rust のみ。他言語アダプタは未着手）。
> preserve 検査へのコールグラフ接続は未実装（判定ルール未策定のため保留）。
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

### Phase 0 (完了) — スタブ + インターフェース確定

- `konpu-cg` クレート新設
- `konpu::analyze::CallGraphProvider` trait 定義 + dummy 実装 (空のコールグラフ)
- `analyze_full` が `Option<&dyn CallGraphProvider>` を受ける追加 API
- 既存 `analyze_full(path, &config)` は dummy provider で委譲

### Phase 1 (完了) — rust-analyzer / SCIP 統合

- LSP のライブ call hierarchy ではなく **`rust-analyzer scip` バッチ**を採用（決定的で堅牢。SCIP は設計 §3 が名指しする中間表現）。
- `konpu-cg::facts` — 抽出器が生成する言語中立な事実モデル（関数・呼び出しサイト・trait 実装・インスタンス化型）。
- `konpu-cg::scip_extract` — SCIP index → `Facts`。rust-analyzer は `is_implementation` relationship を吐かないため、シンボル命名（`Trait#method` / `impl#[Type][Trait]method`）からディスパッチを読む。`facts_from_project()` が rust-analyzer を spawn。
- `call-graph` feature 有効時のみ有効（既定ビルドは依存ゼロ）。

### Phase 2 (完了) — CHA → RTA

- `konpu-cg::graph::CallGraph::build(facts, Precision)`。動的ディスパッチを CHA（trait 全実装）または RTA（インスタンス化された型のみ）で展開。健全な過大近似（偽陽性可・偽陰性なし）。
- 循環（Tarjan SCC）を cross-module（複数ファイル跨ぎ = 真の依存もつれ, actionable）/
  intra-module（単一ファイル内の相互再帰 = 再帰下降パーサ等, 良性）/ self-recursion に3分。
- ハブを fan-out（呼び出し先多 = 分解候補）/ fan-in（呼び出し元多 = 共有ヘルパー・変更集中点）に分離。
- 方針: 良性を分けて actionable な指標だけを前面に出す（oxidtr 実測で cross-module cycle=0 を確認）。
- CLI: `konpu callgraph <path> [--scip FILE] [--precision cha|rta] [--hub-threshold N]`。

### Phase 3（完了）— preserve 検査へのコールグラフ接続

`konpu check --call-graph <path>`（`call-graph` feature）で有効。不変条件:
「`from`→`to` を跨ぐ構造化型 `T` は、`T` の構造が許す代数サーフェス
`{operation, identity, inverse}` を経由してのみ生成・併合される」。

- **検出器 B（集約保存）**: to 層で「複数の `T` を 1 個の `T` に併合する」形の関数
  （`&[T]`→`T` / `T,T`→`T`）が `operation` に到達必須。tree-sitter でシグネチャ判定、
  コールグラフ（RTA）で到達可能性判定。`operation` は SCIP の型修飾シンボル
  `impl#[T]...` で厳密解決。
- **検出器 C（手書きマージ）**: 構築サイトのデータフローで、`T{..}` が ≥2 個の
  `T` 型値を併合しているのに `operation` に到達しない関数を拾う（シグネチャに
  マージが現れない `fn h(a:T,b:T)->Response` 等）。既定 ON。
- **不変条件の 2 軸**: (a) サーフェスは構造 rank 依存、(b) 実効深刻度は law_test の
  有無で調整（検証済み型は据置、未検証は一段降格）。`konpu.toml` の
  `preserve_severity`（off|warn|error）/ `preserve_checks`（aggregate|construct）で調整。

決定不能領域なので既定は warn、証明ではなく疑わしいパターンの検出。best-effort な
箇所（型の末尾セグメント照合、join、到達可能性の見逃し、C の T 変数追跡）は
各所 `// ponytail:` で改善経路を明記。

### Phase 4 以降 — 未着手

- LSP アダプタ差し替えで Kotlin LSP / tsserver 等に対応（事実フォーマットは既に言語中立）。
- RTA の**完全に健全な**精緻化。現状は tree-sitter で値位置の構築サイトを拾い RTA を実際に枝刈りさせているが（layer2 §6.2）、マクロ/serde 由来の構築は見えない。厳密化には StableMIR 級の構築事実が要る。
- preserve 検出器の関数内データフロー化（C の T 変数追跡を params+self からローカル/ループへ拡張）。

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