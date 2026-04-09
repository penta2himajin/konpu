# konpu

Algebraic complexity linter — measures interface composability through algebraic properties.

> 名前の由来：Algebra → Algae → 昆布 → Konpu

## What is this?

Konpu is a linter that measures **algebraic complexity** — how composable your interfaces are. While traditional metrics (cyclomatic complexity, cognitive complexity) measure internal control flow complexity, Konpu measures whether your types and operations satisfy algebraic laws (semigroup, monoid, group, functor, applicative, monad).

See [docs/roadmap.md](docs/roadmap.md) for the full design document.

## Status

Phase 0 — initial development.

## Usage

```bash
cargo run -- check <path>
```

## Development

```bash
cargo build
cargo test
cargo clippy -- -D warnings
```

## Domain Model

The algebraic structure hierarchy and diagnostic rules are formally specified in `models/konpu.als` (Alloy). Use [oxidtr](https://github.com/penta2himajin/oxidtr) to generate Rust types from the model.

## License

MIT
