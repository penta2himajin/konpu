//! CHA/RTA によるディスパッチ解釈と、解決済みコールグラフのクエリ。
//!
//! 設計 (docs/layer2-call-graph-design.md §4): 健全性の向きは
//! 「偽陽性は許容・偽陰性は出さない」に固定した過大近似。
//! - CHA: 動的呼び出しを、その trait の全実装エッジに展開 (最も過大)。
//! - RTA: 実際にインスタンス化された型の実装だけに絞る (CHA より精密で健全)。

use std::collections::{BTreeSet, HashMap};

use crate::facts::{CallTargetKind, Facts, FuncId, TraitMethod};

/// ディスパッチ解決の精度。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precision {
    /// CHA: trait の全実装を候補にする (過大近似)。
    Cha,
    /// RTA: 実際にインスタンス化された型の実装だけに絞る。
    Rta,
}

/// 解決済みコールグラフ。隣接リスト (caller -> callees)。
/// 添字は `Facts::funcs` と同じ空間。
#[derive(Debug, Clone, Default)]
pub struct CallGraph {
    pub edges: Vec<BTreeSet<FuncId>>,
}

impl CallGraph {
    /// 事実からコールグラフを構築する。動的ディスパッチは `precision` に従い展開。
    pub fn build(facts: &Facts, precision: Precision) -> CallGraph {
        // trait メソッド -> その実装群 の索引。
        let mut by_method: HashMap<&TraitMethod, Vec<&crate::facts::ImplEntry>> = HashMap::new();
        for e in &facts.impls {
            by_method.entry(&e.trait_method).or_default().push(e);
        }
        let mut edges = vec![BTreeSet::new(); facts.funcs.len()];
        for c in &facts.calls {
            let bucket = &mut edges[c.caller];
            match &c.target {
                CallTargetKind::Static(f) => {
                    bucket.insert(*f);
                }
                CallTargetKind::Dynamic(tm) => {
                    let Some(impls) = by_method.get(tm) else {
                        continue;
                    };
                    for e in impls {
                        let keep = match precision {
                            Precision::Cha => true,
                            Precision::Rta => facts.instantiated.contains(&e.for_type),
                        };
                        if keep {
                            bucket.insert(e.func);
                        }
                    }
                }
            }
        }
        CallGraph { edges }
    }

    pub fn out_degree(&self, f: FuncId) -> usize {
        self.edges[f].len()
    }

    /// 呼び出される回数 (fan-in)。
    // ponytail: O(V+E) 全走査。ホットになったら逆索引を前計算する。
    pub fn in_degree(&self, f: FuncId) -> usize {
        self.edges.iter().filter(|s| s.contains(&f)).count()
    }

    /// fan-in か fan-out が閾値以上のハブ関数を返す。
    pub fn hubs(&self, min_degree: usize) -> Vec<FuncId> {
        (0..self.edges.len())
            .filter(|&f| self.in_degree(f) >= min_degree || self.out_degree(f) >= min_degree)
            .collect()
    }

    /// 循環を成す強連結成分を返す。サイズ 1 でも自己ループがあれば含める。
    /// 各成分は FuncId 昇順、成分間は最小 FuncId 昇順で安定ソート。
    pub fn cycles(&self) -> Vec<Vec<FuncId>> {
        let mut sccs = Tarjan::run(self);
        // 循環と言えるものだけ: サイズ>1、または自己ループを持つ単一ノード。
        sccs.retain(|scc| scc.len() > 1 || (scc.len() == 1 && self.edges[scc[0]].contains(&scc[0])));
        for scc in &mut sccs {
            scc.sort_unstable();
        }
        sccs.sort_by_key(|scc| scc[0]);
        sccs
    }
}

/// Tarjan の強連結成分分解 (反復スタックで実装)。
struct Tarjan<'a> {
    g: &'a CallGraph,
    index: Vec<usize>,
    low: Vec<usize>,
    on_stack: Vec<bool>,
    stack: Vec<FuncId>,
    counter: usize,
    out: Vec<Vec<FuncId>>,
}

const UNVISITED: usize = usize::MAX;

impl<'a> Tarjan<'a> {
    fn run(g: &'a CallGraph) -> Vec<Vec<FuncId>> {
        let n = g.edges.len();
        let mut t = Tarjan {
            g,
            index: vec![UNVISITED; n],
            low: vec![0; n],
            on_stack: vec![false; n],
            stack: Vec::new(),
            counter: 0,
            out: Vec::new(),
        };
        for v in 0..n {
            if t.index[v] == UNVISITED {
                t.connect(v);
            }
        }
        t.out
    }

    // 明示的ワークリストで再帰を避ける (深いグラフでのスタックオーバーフロー回避)。
    fn connect(&mut self, root: FuncId) {
        // work: (node, 次に見る隣接の添字)。
        let mut work: Vec<(FuncId, usize)> = vec![(root, 0)];
        self.index[root] = self.counter;
        self.low[root] = self.counter;
        self.counter += 1;
        self.stack.push(root);
        self.on_stack[root] = true;

        while let Some(&(v, ni)) = work.last() {
            let neighbors: &BTreeSet<FuncId> = &self.g.edges[v];
            if let Some(&w) = neighbors.iter().nth(ni) {
                work.last_mut().unwrap().1 += 1;
                if self.index[w] == UNVISITED {
                    self.index[w] = self.counter;
                    self.low[w] = self.counter;
                    self.counter += 1;
                    self.stack.push(w);
                    self.on_stack[w] = true;
                    work.push((w, 0));
                } else if self.on_stack[w] {
                    self.low[v] = self.low[v].min(self.index[w]);
                }
            } else {
                // v の隣接を見終わった: 親の low を更新し SCC 根なら pop。
                if self.low[v] == self.index[v] {
                    let mut scc = Vec::new();
                    loop {
                        let w = self.stack.pop().unwrap();
                        self.on_stack[w] = false;
                        scc.push(w);
                        if w == v {
                            break;
                        }
                    }
                    self.out.push(scc);
                }
                work.pop();
                if let Some(&(parent, _)) = work.last() {
                    self.low[parent] = self.low[parent].min(self.low[v]);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facts::{CallSite, ImplEntry, TraitMethod};

    // a -> b (static), a -> c (static)
    fn linear_facts() -> Facts {
        let mut f = Facts::default();
        let a = f.add_func("a", "src/a.rs", 1);
        let b = f.add_func("b", "src/b.rs", 1);
        let c = f.add_func("c", "src/c.rs", 1);
        f.calls.push(CallSite {
            caller: a,
            target: CallTargetKind::Static(b),
        });
        f.calls.push(CallSite {
            caller: a,
            target: CallTargetKind::Static(c),
        });
        f
    }

    #[test]
    fn static_edges_are_direct() {
        let f = linear_facts();
        let g = CallGraph::build(&f, Precision::Cha);
        assert_eq!(g.out_degree(0), 2);
        assert_eq!(g.in_degree(1), 1);
        assert!(g.cycles().is_empty());
    }

    // caller dispatches dyn Trait::m; two impls (Money, Point). Money instantiated only.
    fn dyn_facts() -> Facts {
        let mut f = Facts::default();
        let caller = f.add_func("caller", "src/c.rs", 1);
        let money_impl = f.add_func("Money::m", "src/money.rs", 1);
        let point_impl = f.add_func("Point::m", "src/point.rs", 1);
        let tm = TraitMethod::new("Trait", "m");
        f.calls.push(CallSite {
            caller,
            target: CallTargetKind::Dynamic(tm.clone()),
        });
        f.impls.push(ImplEntry {
            trait_method: tm.clone(),
            for_type: "Money".into(),
            func: money_impl,
        });
        f.impls.push(ImplEntry {
            trait_method: tm,
            for_type: "Point".into(),
            func: point_impl,
        });
        f.instantiated.insert("Money".into());
        f
    }

    #[test]
    fn cha_expands_to_all_impls() {
        let f = dyn_facts();
        let g = CallGraph::build(&f, Precision::Cha);
        // caller -> both impls
        assert_eq!(g.out_degree(0), 2);
    }

    #[test]
    fn rta_keeps_only_instantiated() {
        let f = dyn_facts();
        let g = CallGraph::build(&f, Precision::Rta);
        // caller -> Money::m only (Point never instantiated)
        assert_eq!(g.out_degree(0), 1);
        assert!(g.edges[0].contains(&1)); // money_impl id = 1
        assert!(!g.edges[0].contains(&2)); // point_impl id = 2
    }

    #[test]
    fn detects_self_loop_and_cycle() {
        let mut f = Facts::default();
        let a = f.add_func("a", "src/a.rs", 1);
        let b = f.add_func("b", "src/b.rs", 1);
        let s = f.add_func("s", "src/s.rs", 1);
        // a <-> b cycle
        f.calls.push(CallSite { caller: a, target: CallTargetKind::Static(b) });
        f.calls.push(CallSite { caller: b, target: CallTargetKind::Static(a) });
        // s self-loop
        f.calls.push(CallSite { caller: s, target: CallTargetKind::Static(s) });
        let g = CallGraph::build(&f, Precision::Cha);
        let cycles = g.cycles();
        assert_eq!(cycles.len(), 2);
        assert_eq!(cycles[0], vec![a, b]);
        assert_eq!(cycles[1], vec![s]);
    }

    #[test]
    fn hubs_by_degree() {
        // one callee with high fan-in.
        let mut f = Facts::default();
        let hub = f.add_func("hub", "src/h.rs", 1);
        let callers: Vec<_> = (0..4).map(|i| f.add_func(format!("c{i}"), "src/c.rs", i)).collect();
        for &c in &callers {
            f.calls.push(CallSite { caller: c, target: CallTargetKind::Static(hub) });
        }
        let g = CallGraph::build(&f, Precision::Cha);
        assert_eq!(g.in_degree(hub), 4);
        assert!(g.hubs(4).contains(&hub));
        assert!(!g.hubs(5).contains(&hub));
    }
}
