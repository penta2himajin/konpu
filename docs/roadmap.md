# Konpu — 代数的複雑度リンター ロードマップ

> 名前の由来：Algebra → Algae → 昆布 → Konpu
> 作成日: 2026-03-29

---

## 1. Konpuとは何か

Konpuは、コードの**代数的複雑度**（algebraic complexity）を静的に検査するリンターである。

### コードの「扱いやすさ」を構成する層

コードベースが「扱いやすい」かどうかは、分析の粒度が異なる複数の独立した層で決まる。

| 層 | スコープ | 問い | 既存ツール／状態 |
|----|----------|------|-----------|
| 1. イントラノード複雑度 | 関数内の制御フロー | **読めるか** — 分岐・ループ・ネストの複雑さ | 循環的複雑度、認知的複雑度（clippy, eslint等）— 既存ツールがカバー済み |
| 2a. モジュール依存グラフ複雑度 | モジュール/クレート間のimport関係 | **差し替えられるか** — このコンパイル単位を変更したら何が巻き込まれるか | 部分的（coupling/cohesionメトリクス等）— 体系的なツールは未確立 |
| 2b. コールグラフトポロジ複雑度 | 関数間の実際の呼び出し関係 | **追えるか** — この関数を変更したら影響はどこまで波及するか | ほぼ未開拓。動的ディスパッチの解決が本質的な難所 |
| 3. インターフェース代数的複雑度 | 型と操作が満たす代数的性質 | **組めるか** — この部品は他と合成できるか | ほぼ未開拓。Konpuの現在の主眼 |

四層とも良好でないとコードベースは「扱いやすい」とは言えない。既存ツールチェーンは第1層のカバレッジだけが充実していて、第2層（2a/2b）は断片的、第3層はほぼ未開拓。

**第2層は独立に悪化しうる2つの異なる粒度に分かれる。** 2a（モジュール依存グラフ）はノードがモジュール/クレート、エッジがimport関係。2b（コールグラフ）はノードが関数、エッジが実際の呼び出し。依存関係グラフがきれいでもモジュール内の関数呼び出しが絡まっていることがあり、逆に依存関係が循環していても実際に呼ばれる関数はごく一部ということもある。実装難易度も大きく異なり、2aは構文解析（use/import文の抽出）でほぼ実現できるのに対し、2bは動的ディスパッチ（trait objectやジェネリクス経由の呼び出し）の解決に意味解析が必須で、完全な解決は原理的に不可能（決定不能）。健全な近似に留める設計が要る。詳細は`docs/layer2-call-graph-design.md`を参照。

**Konpuは現時点では第3層（インターフェース代数的複雑度）に集中する。** 第1層は既存ツール（clippy、eslint等）に委ね、第2層（2a/2b）は将来的な拡張として位置づける。長期的には全層をKonpuでカバーすることを目指すが、まずは最も欠落している第3層から着手する。

### 情報源による実現可能性

各層をどの情報源から構築できるかは層ごとに大きく異なり、この差が多言語対応（Phase 2）のアーキテクチャに直接影響する。

| 層 | tree-sitterのみで可能か | 必要な情報源 |
|----|------------------------|------------|
| 1. イントラノード | 可能 | 構文情報のみ（分岐・ループ・ネストのカウント） |
| 2a. モジュール依存グラフ | ほぼ可能 | 構文情報＋言語のモジュール解決規約（re-export・glob import・条件付きコンパイルで精度が落ちる） |
| 2b. コールグラフ | 不可能 | コンパイラ/インタプリタの意味解析情報（型階層、trait実装一覧、単相化後のMIR等）が必須。それでも完全な解決は決定不能で、健全な過大近似（RTA等）に留まる |
| 3. 代数的複雑度 | 部分的に可能 | 存在検査・型シグネチャ検査は構文情報＋proc macro（コンパイラ統合）で可能。法則の充足そのものは静的に決定不能（ライスの定理）。詳細は`docs/layer3-decidability-limits.md`を参照 |

第1層と2aは既存のtree-sitterベースの解析基盤でほぼ実現できるが、2bと3の一部（跨ファイル名前解決を要る単位元検索、文脈伝播度計測、法則充足そのもの）は言語ごとのコンパイラ統合が必要になる。「tree-sitter grammarを追加すれば言語が増える」という単純な多言語対応シナリオは第2b層・第3層の一部には成立しない。

### 背景にある洞察

プログラミングにおいて「良く出来ている」「扱いやすい」と感じるインターフェースには共通の代数構造がある。多くは半群をなし、大抵はモノイドで、まれに群やモナドになる。この「プロの感覚」を定量化し、Lintとして機械的に検査可能にすることがKonpuの目的である。

AIが生成したコードを人間がメンテナンスするケースにおいて特に有用。「動くが合成できないAPI」を構造的に弾くことで、コードベースの設計品質をガードレールとして維持する。

---

## 2. 代数的複雑度の4軸

### 軸1：構造ランク（Structural Rank）

型`T`と合成操作`·`のペアが満たす代数構造の最上位を順序値として表現する。

| ランク | 構造 | 性質 | 嬉しさ |
|--------|------|------|--------|
| 0 | マグマ | 合成可能だが法則なし | 組み合わせられるが予測不能 |
| 1 | 半群 | 結合律 | `reduce`/`fold`可能、並列分割統治可能 |
| 2 | モノイド | 結合律＋単位元 | 空の場合が自然に扱える、条件付き挿入/除外が安全 |
| 3 | 群 | 結合律＋単位元＋逆元 | Undo/Redo、差分計算、双方向変換 |

高階構造（ファンクタ、アプリカティブ、モナド）も別軸で0〜3のランクを付与する。

| ランク | 構造 | 性質 | 嬉しさ |
|--------|------|------|--------|
| 0 | ― | 文脈なし | ― |
| 1 | ファンクタ | 構造を保つ写像（map） | 文脈を気にせず中身だけ変換できる |
| 2 | アプリカティブ | 独立した文脈の合成 | 並列バリデーション、全エラー収集 |
| 3 | モナド | 依存する文脈の逐次合成 | 分岐を含む逐次処理がパイプラインで書ける |

**注意：ランクが高いほど良いわけではない。** モナドは逐次依存を表現できる代わりに並列合成を失う。アプリカティブは依存を表現できないからこそ並列が可能。「この操作に必要十分な構造は何か」を見極めることが設計判断の核。

### 軸2：充足ギャップ（Compliance Gap）

ある`(型, 操作)`ペアが目標構造の法則をどの程度満たしているかの逸脱度。

**定義：** 目標構造が要求する法則の集合 `L = {l₁, l₂, ..., lₙ}` に対して、各法則 `lᵢ` のテスト充足率 `pᵢ ∈ [0, 1]` を測定し、充足ギャップを `G = 1 - (Σpᵢ / n)` とする。G=0なら完全充足、G=1なら全法則が破綻。

Konpuは法則テストの存在と通過を検査する（テスト生成はスコープ外。後述）。

### 軸3：合成コスト比（Composition Cost Ratio）【メタデータ扱い】

n個の要素の合成コスト`C(n)`に対して、合成コスト比を`R(n) = C(n) / n`とする。

代数的性質というよりパフォーマンス特性の問題であるため、Konpuの検査項目としてではなくアノテーションのメタデータとして記録する。計測自体はベンチマークツール（criterion等）に委ねる。

### 軸4：文脈伝播度（Context Propagation Degree）

文脈型`F<T>`の伝播度`P(F)`を、`F`の文脈部分が取りうる状態の構造的サイズとして定義する。

| 型 | 文脈 | P |
|----|------|---|
| `Option<T>` | `{None, Some}`の区別 | 1 |
| `Result<T, E>` (Eがenum) | Eのバリアント | `variants(E)` |
| `Vec<T>` | 長さと順序 | ∞（非有界） |
| `State<S, T>` | Sの型 | `size(S)` |

**計測方法：** 型定義から構造的サイズを機械的に算出する。enumならバリアント数、structならフィールドの直積、再帰型やコレクション型は非有界として∞。完全に静的な解析で計測可能。

**段階的精緻化：** 初期はバリアント数のみ。将来的にペイロードの深さを加味する重み付けを追加する余地を残す。

---

## 3. コア設計

### 3.1 Konpuの責務

Konpuはリンターであり、テストランナーでもテストジェネレーターでもない。

**Konpuがやること：**
- アノテーション付きの`(型, 操作, 目標構造)`を検出する
- 静的解析で構造的な違反を検出してエラー/ワーニングを報告する
- 法則テストの存在と通過を検査する（テスト自体はユーザーまたは外部ツールが書く）
- 文脈伝播度を静的に計測し、設定された上限との比較を報告する
- テンプレートに基づくディレクトリ/モジュール単位の期待構造の検査

**Konpuがやらないこと：**
- テストコードの生成（`konpu scaffold`で補助的にスケルトンを提供するが、コア責務ではない）
- テストの実行（CI結果を参照する）
- ベンチマーク計測（軸3は外部ツールに委ねる）

### 3.2 静的に検出できるもの

以下はコードを実行せずに型シグネチャとインターフェース構造から判定可能で、Lint（エラー/ワーニング）として報告する。

- モノイドを宣言しているが単位元（identity関数やDefault実装）が未定義
- 二項演算の型シグネチャが`(T, T) -> T`の閉包を満たしていない
- 群を宣言しているが逆元操作が存在しない
- ファンクタを宣言しているがmapの型シグネチャが構造保存の形になっていない
- 文脈伝播度が設定上限を超過している

加えて、結合律については静的な推論で「成り立つための必要条件」を検査する。

- 操作が純粋であること（`&mut self`を取らない、外部状態に触らない）
- 入出力型が閉じていること
- 既知の結合律違反パターン（浮動小数点算術の直接使用等）に非該当

全条件を通過した場合はconfidenceレベルのinfoとして報告し、確実な検証が必要なら法則テストで確定する。

**この静的検査には原理的な限界がある。** 検出できるのは「法則を満たすための必要条件」までで、法則の充足そのもの（結合律・単位律・ファンクタ則）は一般に静的判定不能である。詳細と理由は`docs/layer3-decidability-limits.md`を参照。

### 3.3 静的に検出できないもの（法則テストの存在・通過を検査）

以下は実行時の振る舞いの問題であり、静的には判定不能。

- 結合律：`(a·b)·c == a·(b·c)`
- 単位律：`a·e == a` かつ `e·a == a`
- ファンクタ則：`map(f∘g) == map(f)∘map(g)`

Konpuはこれらについて**テストが存在し、CIで通過していること**を検査する。

- テストの発見はアノテーション（`#[konpu::law(associativity)]`等）により行う
- CI通過の確認はテスト実行結果のメタデータ参照（具体的な方式はPhase 1で設計）
- テストが存在しない場合はワーニングを出す

テストの記述自体はユーザーの責任。`konpu scaffold`でproperty-based testのスケルトンを生成する補助機能を提供するが、あくまでオプション。

**注意：法則テストの通過も「証明」ではない。** property-based testingは有限個のサンプルに対する反証失敗であり、境界条件でのみ法則が破れるケース（`docs/layer3-decidability-limits.md`のop5例を参照）はランダムサンプリングをすり抜けうる。充足ギャップGは「テスト充足率」であって「法則の真の充足度」ではない点を設計上の前提として受け入れている。

### 3.4 アノテーション設計

```rust
// 基本形：型と操作に対して目標構造を宣言
#[konpu::monoid(op = "compose", identity = "empty")]
struct Middleware { ... }

// 合成コスト比はメタデータとして記録
#[konpu::monoid(op = "concat", identity = "empty", cost = "linear")]
struct LogEntries { ... }

// 法則テストの紐付け
#[konpu::law(associativity)]
#[test]
fn test_middleware_associativity() { ... }

// 無視する場合は理由を構造的に分類
#[konpu::ignore(reason = "intentional", note = "適用順序がビジネス上の意味を持つ")]
fn apply_discounts(...) { ... }

// 将来的な対応予定を示す
#[konpu::ignore(reason = "debt", note = "モノイドへのリファクタリング予定")]
fn merge_configs(...) { ... }
```

ignoreの`reason`は以下の値を取る。
- `intentional`：意図的な設計判断による逸脱
- `debt`：技術的負債（将来の解消予定）
- `infeasible`：技術的に代数構造の適用が不可能

### 3.5 テンプレート設計

設定ファイル（`konpu.toml`）でディレクトリ/モジュール単位の期待構造を宣言する。

```toml
[defaults]
max_propagation = 8  # 文脈伝播度のデフォルト上限

# ディレクトリ単位の期待構造
[layers.domain]
path = "src/domain/**"
expect = ["monoid", "group"]  # この層のValue Objectはモノイド以上を期待
max_propagation = 4

[layers.application]
path = "src/application/**"
expect = ["monad"]  # ユースケース層はモナド的合成を期待
max_propagation = 8

[layers.infra]
path = "src/infra/**"
expect = ["functor"]  # インフラ層はファンクタ則を期待
max_propagation = -1  # 無制限

# 層間制約（Phase 2）
# [boundaries.domain_to_infra]
# from = "src/domain/**"
# to = "src/infra/**"
# preserve = ["monoid"]  # ドメインのモノイド則がインフラ層通過後も保存されること
```

プリセットを用意する。

```toml
# プリセット指定
preset = "ddd"  # "ddd" | "hexagonal" | "clean"

# プリセットの上書き
[layers.domain]
max_propagation = 6  # プリセットのデフォルトを上書き
```

### 3.6 既存プロジェクトへの導入

大量の違反が出るノイズを避けるため、ベースラインモードを提供する。

```bash
# 現状を全てベースラインとして記録
konpu baseline

# 以降、新規の逸脱のみ検出
konpu check  # ベースライン以降の変更のみ報告
```

### 3.7 多言語対応方針

tree-sitterをパーサーとして採用し、言語非依存のコア解析の上に言語別アダプタを載せる。

- 最下層：コア解析ライブラリ（`(型, 操作)`ペアの抽出、構造候補推定、法則テンプレート）— Rust実装
- 中間層：言語別アダプタ（tree-sitter grammer経由で各言語のASTをコア表現に変換）
- 最上層：CLIおよびLSPサーバー

初期対応はRustのみ。Kotlin、TypeScriptは後続フェーズで追加。

**注意：この構成が有効なのは第1層・第2a層・第3層の一部（存在検査・型シグネチャ検査）に限られる。** 第3層の法則充足に近い検査（跨ファイル名前解決を要する単位元検索や文脈伝播度計測）や第2b層（コールグラフ）は、tree-sitter grammarの追加だけでは対応できず、言語ごとの意味解析層（Rustならproc macro/コンパイラ統合、Kotlinならcompiler plugin、TypeScriptならcompiler API）が別途必要になる。詳細はセクション1「情報源による実現可能性」を参照。

---

## 4. oxidtrとの連携

### 4.1 責務分離

| ツール | 責務 |
|--------|------|
| oxidtr | Alloyモデル → コード + アノテーション生成 |
| Konpu | アノテーション → 静的検査 + テスト存在・通過検査 + 計測レポート |

### 4.2 接続インターフェース

oxidtrがAlloyモデルの代数的性質に基づいてKonpuアノテーションを生成コードに自動付与する。Konpuはアノテーションを読んで検査を行う。接続はアノテーションのフォーマットのみが契約であり、疎結合。

oxidtrが法則テスト（property-based test）を生成する場合、`#[konpu::law(...)]`アノテーションを付与することで、Konpuがそのテストの存在と通過を検査する。テスト生成の責務はoxidtr側にあり、Konpuはその結果を参照するのみ。

### 4.3 AI生成コードへの適用フロー

```
oxidtr: Alloyモデル → Rust/Kotlin/TS コード + Konpuアノテーション + 法則テスト
  ↓
AI（Claude Code等）: アノテーション付きの型・操作の実装を生成
  ↓
konpu check: 静的Lintで構造的違反を即座に検出
  ↓
CI: 法則テスト実行
  ↓
konpu check: 法則テストの通過を確認
  ↓
違反があればAIに制約付きエラーメッセージをフィードバックして再生成
```

---

## 5. ロードマップ

### Phase 0：最小CLI（Rust対象のみ）

**目的：** 「アノテーションを書いて`konpu check`を実行すると構造的な違反が検出される」という最小体験を作る。

#### 0-A：アノテーション仕様の確定
- [ ] `#[konpu::*]`アノテーション群の仕様策定（目標構造宣言、法則テスト紐付け、ignore）
- [ ] Rustのproc macroとして実装（アノテーションの解析のみ、コード変換はしない）

#### 0-B：静的解析コア
- [ ] tree-sitter-rustで`(型, 操作)`ペアの抽出
- [ ] 単位元の存在検査（Default実装、identity関数の検出）
- [ ] 型シグネチャの閉包検査（`(T, T) -> T`形式の確認）
- [ ] 逆元操作の存在検査
- [ ] 結合律の必要条件検査（純粋性、閉包性、既知違反パターンの非該当）

#### 0-C：CLI + レポート
- [ ] `konpu check <path>` — 指定パスに対して静的解析を実行、違反を報告
- [ ] エラー/ワーニング/info の3段階出力
- [ ] 終了コードによるCI統合（違反があれば非ゼロ）

**完了基準：** 自分のRustコードに`#[konpu::monoid(...)]`を付けて`konpu check`を実行し、単位元の欠落や型シグネチャの不整合が検出されること。

---

### Phase 1：テスト検査 + 文脈伝播度 + テンプレート

**目的：** 軸2（充足ギャップ）と軸4（文脈伝播度）の計測を実装し、テンプレートによるアーキテクチャレベルの制約を導入する。

#### 1-A：法則テストの存在・通過検査
- [x] `#[konpu::law(...)]`アノテーションによるテスト発見メカニズム
- [x] CI実行結果の参照方式設計（`konpu check --test-results <captured cargo test output>` で `failures:` を読む。konpu はテストを走らせず結果を参照するだけ）
- [x] テスト未存在時のワーニング出力（`MissingLawTest`）
- [x] テスト不通過時のエラー出力（`FailingLawTest`）＋ 充足ギャップ G の数値レポート化（`konpu report [--test-results] [--infer]` が per-structure / overall の gap = 1 - passing/required を表示）

#### 1-B：文脈伝播度の計測
- [x] 型定義からの構造的サイズ算出（enum → バリアント数、struct → フィールド直積）（`src/analyze/propagation.rs`）
- [x] 再帰型・コレクション型の∞判定（相互再帰も含めて検出、テスト済み）
- [x] `max_propagation`設定との比較、超過時のワーニング（`check::check_propagation` → `PropagationExceeded`）

#### 1-C：テンプレート設定
- [x] `konpu.toml`のパーサー実装（`src/analyze/template/mod.rs`）
- [x] ディレクトリ単位の期待構造マッピング（`match_layer`、`**`/`*` glob対応）
- [ ] プリセット（`ddd`, `hexagonal`, `clean`）— **`ddd`のみ実装**。`hexagonal`/`clean`は`preset_layers()`が空リストを返すスタブのまま
- [x] ignoreアノテーションの理由分類（`intentional` / `debt` / `infeasible`）（`konpu-macros`のproc macroでreason値をバリデーション。`konpu report`が理由別に集計）
- [x] ベースラインモード（`konpu baseline`）（`src/analyze/baseline.rs`、CLI `Baseline`サブコマンド）

#### 1-D：補助機能
- [x] `konpu scaffold` — 法則テストのスケルトン生成（`src/analyze/scaffold.rs`。Rust/proptest形式に加えSwift(XCTest)/Kotlin(kotlin.test)/TS(jest/vitest)にも対応）
- [x] `konpu report` — コードベース全体の代数的複雑度サマリー出力（充足ギャップ、ignore理由別集計、層別期待構造mismatch、境界違反まで出力）

**完了基準：** `konpu.toml`にDDDプリセットを設定し、ドメイン層でモノイド則のテストが欠落している型がワーニングとして報告されること。文脈伝播度の超過が検出されること。→ 達成（プリセットは`ddd`のみ実装）。

---

### Phase 2：層間制約 + 多言語対応

**目的：** アーキテクチャ全体の代数的整合性を検証可能にし、対象言語を拡張する。

#### 2-A：層間制約
- [x] `konpu.toml`の`[boundaries]`セクション実装（`from`/`to`/`preserve`/`preserve_severity`/`preserve_checks`）
- [x] 「ドメイン層のモノイド則がインフラ層通過後も保存されること」の検査（`src/analyze/preserve_cg.rs`。`konpu check --call-graph`、`call-graph` featureが必要）
- [x] 層間テストの存在・通過検査（`preserve_cg::check_preserve`が`law_tests`を参照して判定。逆方向import違反自体はcall-graph機能なしでも`analyze_full`の`boundary_violations`で検出）

#### 2-B：多言語対応（2言語目 = Swift）
言語シームを導入（`parser::Language` + `FileExtract`）。解析エンジン（check/infer/
template/compliance）は言語非依存で、言語別は抽出層（`extract` / `extract_swift`）のみ。
- [x] Swift アダプタ（tree-sitter-swift）: struct/class/enum/extension→ImplInfo、
  function_declaration→MethodInfo（static=関連関数, mutating=&mut self）、
  `static let zero`→単位元正規化、伝播度（struct field/enum variant）
- [x] Swift アノテーション規約 = 推論優先 + `// konpu:` コメント
  （monoid/semigroup/group/magma 宣言・law(<laws>)・ignore）
- [x] `--test-results` が `swift test`(XCTest) 出力も解釈（FailingLawTest/compliance）
- [x] 演算子メソッド（`+`→add / `*`→mul）・`AdditiveArithmetic` 準拠→合成 add+zero で Monoid 推論
- [x] `[T]`/`T?`/`Set` 等の propagation 正規化・higher-kinded コメント注釈（`higher: functor`）・
  Swift 版 scaffold（XCTest + `// konpu: law`）。**layer-3 代数機能は Rust とフルパリティ**。
- [x] 境界の逆方向 import 検査（module→層マッピング）: `[boundaries.*].from_modules` に
  `from` 層の Swift モジュール名を宣言。`to` パスのファイルがそのモジュールを import したら違反。
  `UseStatement.language` で Rust（パスキー照合）と Swift（モジュール名照合）を切替。**Swift は境界も含めフルパリティ**。
- [x] Kotlin（3言語目、tree-sitter-kotlin-ng）: layer-3 フルパリティ。
  推論（class/data class/interface/object、`operator fun plus`→add / `times`→mul、
  `companion object` の `fun zero()`/`val zero`→単位元）、`// konpu:` コメント注釈
  （共有 `directive` モジュール）、law + `--test-results`（Gradle `Class > test FAILED`）、
  compliance、propagation（List/Set/Map/`T?` 正規化）、scaffold（kotlin.test）、
  逆import境界（完全修飾 import を `from_modules` 接頭辞照合）、
  call graph（layer 2b: 循環/ハブ + preserve B/C、`call_graph_kotlin` に精密解決移植）。
  **Kotlin は Swift と完全同等**（layer-3 + layer-2b）。CLI は `cg_ts_language` で
  Rust/SCIP・Swift・Kotlin を 3-way 判定。
- [x] TypeScript（4言語目、tree-sitter-typescript）: layer-3 フルパリティ。
  推論（class/abstract class/interface/enum、演算子オーバーロード無しなので名前付き
  `combine`/`merge`、`static zero()` / `static readonly zero: T`→単位元＝Kotlin companion 相当）、
  `// konpu:` コメント注釈（共有 `directive`、`export class` は `export_statement` を剥がして到達）、
  law + `test("name")`/`it(...)` テスト名抽出、compliance、propagation（`T[]`/`Array`/`Set`/`Map` 正規化）、
  scaffold（`.laws.test.ts`、jest/vitest 形式 + `// konpu: law`）、
  逆import境界（import 指定子を `from_modules` 接頭辞照合）、
  call graph（layer 2b: `src/analyze/call_graph/ts.rs`。`new T(...)`=構築、`a.foo()`/`this.a.foo()`
  =member_expression。TS はインスタンスメンバを常に `this.` 修飾するので、`resolve_receiver` が
  `this`/ローカル/`this.<field>`/`Type.` を再帰的に型解決。callable-value 規約は無いので bare 呼びは
  self→自由関数→Dynamic。preserve B/C も動作）。**Swift/Kotlin と完全同等**（layer-3 + layer-2b）。
  言語別実装は `extract/ts.rs` + `call_graph/ts.rs` のみ、解析エンジンは無改変。
  CLI は `cg_ts_language` で Rust/SCIP・Swift・Kotlin・TS を 4-way 判定。

#### 2-C：oxidtr正式連携
- [ ] oxidtrが生成するコードへのKonpuアノテーション自動付与
- [ ] oxidtrが生成する法則テストへの`#[konpu::law]`自動付与

---

### Phase 3：LSP + 候補検出

**目的：** エディタ上でのリアルタイムフィードバックと、アノテーションなしの代数構造候補検出。

#### 3-A：LSPサーバー
- [ ] 静的解析結果のインラインdiagnostics表示
- [ ] 文脈伝播度のcode lens表示
- [ ] Quick Fix提案（「単位元を追加」「ignoreアノテーションを追加」等）

#### 3-B：アノテーションなしの候補検出
- [ ] 二項演算を持つ型の自動列挙
- [ ] 型シグネチャからの代数構造候補推定
- [ ] 「この型はモノイド候補だが単位元が未定義」等のinfo診断

---

## 6. 依存グラフ

```
Phase 0-A (アノテーション仕様)
  └─► Phase 0-B (静的解析コア) / Phase 1-A (テスト検査) / Phase 2-C (oxidtr連携)

Phase 0-B (静的解析コア)
  └─► Phase 0-C (CLI)

Phase 0-C (CLI)
  └─► Phase 1 (全体の前提)

Phase 1-C (テンプレート)
  └─► Phase 2-A (層間制約)

Phase 0-B (tree-sitter基盤)
  └─► Phase 2-B (多言語対応)

Phase 1 完了
  └─► Phase 3 (LSP)
```

---

## 7. アーキテクチャ層と期待される代数的性質

テンプレートプリセットの設計根拠。

| 層 | 期待される性質 | 根拠 |
|----|--------------|------|
| ドメイン層 | モノイド、群 | ビジネスルールは「値の合成と変換の法則」。Value Objectの不変性が代数的法則の前提条件 |
| アプリケーション層 | モナド、アプリカティブ | ユースケースは逐次処理の合成。バリデーションはアプリカティブ（全エラー収集）、ワークフローはモナド（短絡評価） |
| インフラ層 | ファンクタ | リポジトリ/ゲートウェイの責務は「構造を保ったまま変換」。ファンクタ則の充足がアダプタ差し替え可能性を保証 |
| 層間接続 | ファンクタ（準同型写像） | ドメイン層の法則がインフラ層通過後も保存されることがアーキテクチャの健全性条件 |

---

## 8. 保留事項

| 項目 | 状態 |
|------|------|
| 軸3（合成コスト比）の扱い | メタデータとしてアノテーションに記録。Konpuでの計測はスコープ外 |
| 軸4の精緻化（ペイロード深さの重み付け） | Phase 1ではバリアント数のみ。精緻化はPhase 2以降で検討 |
| ignoreの`reason`分類のレポート統合 | `intentional` vs `debt`の集計レポートはPhase 1-Dで対応 |
| CI結果参照の具体方式 | Phase 1-Aで設計。候補：JUnit XML / テスト結果JSON / テストランナー直接連携 |
| Alloyモデルからの代数的法則assertion生成 | oxidtr側の拡張として検討。Konpuのスコープ外 |
| Web UIダッシュボード | 未計画。CLIレポートで当面は十分 |
| 第2層b（コールグラフ）の実装方針 | RTAベースの健全な過大近似（偽陽性は許容、偽陰性は出さない）で実装する方針を確定。事実抽出（コンパイラ/StableMIR統合）と解釈（ディスパッチ意味論）を分離するアーキテクチャ。Konpu本体（Phase 0〜）とは別の兄弟ツールとして構想し、事実フォーマットの仕様策定のみ先行させる。詳細は`docs/layer2-call-graph-design.md` |
| 第2層a（モジュール依存グラフ）の位置づけ | 2bより実装コストが低く（構文解析でほぼ実現可）、konpu.tomlの層別テンプレートと自然に統合できるため、第2層の中では先に着手する候補。ロードマップ上のPhase番号は未確定 |
| 第3層の静的判定の原理的限界 | 法則充足の判定はライスの定理により決定不能。Konpuは必要条件の静的検査＋法則テストへの委譲という設計で対応する（3.2/3.3節に反映済み）。詳細は`docs/layer3-decidability-limits.md` |
