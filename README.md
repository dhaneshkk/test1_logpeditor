# IOGP rule editor — controlled English → FOL → detector configuration

End users write IOGP Life-Saving Rule alerts as controlled-English sentences. Other applications
receive the rules as time-indexed first-order logic plus a JSON detector configuration:

```
If a worker is in a height-zone and does not wear a harness for at least 3 seconds
then VisionGuard raises a critical working-at-height-alert for the worker.
```

| Crate | What it is |
|---|---|
| [`ape-rs/`](ape-rs) | The Attempto Parsing Engine (APE) running on Scryer Prolog (the fork at `dhaneshkk/scryer-prolog`, branch `fix/ape-rs-findings`) |
| [`acewiki-rs/`](acewiki-rs) | AceWiki in Rust on ape-rs; provides the chart parser used for predictive editing |
| [`iogp-editor/`](iogp-editor) | The rule editor: predictive editor with auto-suggestion, APE-checked compiler to FOL/JSON, approval workflow with four-eyes sign-off and a hash-chained audit log, clingo-based rule review, real-site replay, OpenAI-compatible `/v1/chat/completions` |
| [`iogp-editor/iogp-runtime/`](iogp-editor/iogp-runtime) | **The inference library** the detection pipeline links: loads `config.json`, evaluates frames (2–40 µs per frame), Rust API and C ABI, depends only on serde |
| [`notebooks/`](notebooks) | A single-cell Colab notebook that builds and verifies everything: tests, MCC, Lean proofs, editor and API checks, clingo review, runtime benchmark |

Formal guarantees are in `iogp-editor/formal/IogpAlerts.lean` (33 theorems, Lean 4.15/4.25).
Each crate's README has the details.

## Requirements

* **Rust 1.98**, pinned by `rust-toolchain.toml` in the root and in each crate
* **CMake ≥ 3.16** and a C/C++ compiler (clingo is built from source)
* **git** (to fetch the Scryer fork)

```bash
sudo apt install build-essential cmake git      # Ubuntu / WSL
rustup toolchain install 1.98
cd iogp-editor && ./scripts/check-toolchain.sh
```

## Build and run

```bash
cd iogp-editor
cargo build --release
./target/release/iogp --prolog-dir ../ape-rs/prolog-scryer \
    serve --addr 127.0.0.1:9191 --rules ~/iogp/iogp_rules.json [--users users.json]
# open http://localhost:9191
cargo test --release                                  # editor tests (APE, clingo, grammar, workflow)
(cd iogp-runtime && cargo test --release)             # fast engine vs reference on 72k frames, C ABI
```

The crates use relative path dependencies (`iogp-editor` → `../ape-rs`, `../acewiki-rs`), so keep
this layout. `iogp-editor/tools/yappu_parity.py` compares against yappu-fsm's own engine and
needs a checkout of yappu-fsm passed as its first argument.

## Licence

LGPL-3.0-or-later. APE and AceWiki are LGPL-3.0, Scryer Prolog is BSD-3-Clause, and clingo,
through clingox, is MIT.
