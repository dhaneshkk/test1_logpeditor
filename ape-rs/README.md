# ape-rs — Attempto Parsing Engine on Rust + Scryer Prolog

> Toolchain: Rust **1.98**, pinned in `rust-toolchain.toml` (shared with iogp-editor; rustup installs it on first use).

APE (ACE → DRS, FOL, PNF, TPTP, OWL 2, SWRL, RuleML, ACE paraphrases) migrated off SWI-Prolog:

* **Rust host** (`src/`): library API, CLI (`ape`), HTTP server with the classic APE web-service
  parameters **and** an OpenAI-compatible `POST /v1/chat/completions` (`model: "ape-<output>"`).
  Replaces SWI's `qsave_program`, `thread_httpd`, `http_dispatch`, `http_cors`, socket server,
  memory files, `sgml_write`, and `call_with_time_limit`.
* **`ape-parity`** (`src/bin/ape_parity.rs`): differential harness, MCC scoring, fixture generator
  and import-cycle checker (replaces the former Python tooling).
* **Linguistic core on Scryer** (`prolog-scryer/`): APE's tokenizer, ACE grammar, anaphora
  resolution and all DRS translators, mechanically converted by `tools/swi2scryer.pl` plus a small
  compatibility layer (`prolog-overrides/swi_compat.pl`) and two hand-ported modules
  (`xmlterm_to_xmlatom.pl`, `serialize_term.pl`).

## Verified parity with SWI-Prolog APE

| Check | Result |
|---|---|
| APE regression suite (3,732 texts): DRS structurally identical (`=@=`) | **3,732 / 3,732** |
| ACE acceptance, Scryer vs SWI (MCC) | **1.000** (TP 2,682, TN 1,050, FP 0, FN 0) |
| Acceptance vs gold, SWI and Scryer (MCC, identical) | 0.914 (same 130 known APE regressions) |
| 22 output modes × 375 texts: byte-identical | 7,597 / 8,250 |
| … equivalent after normalising SWI's random OWL ids, raw `_G123` var names, error-term wording | **8,250 / 8,250** |
| `cargo test`: 515 golden SWI outputs byte-identical | pass |
| Lean 4 (`formal/ApeMigration.lean`): quoting round-trip, XML indent width, case mapping, MCC | checked, no `sorry` |

## Usage

```bash
# regenerate the Scryer tree from upstream APE (needs swipl only for this step)
swipl tools/swi2scryer.pl /path/to/APE prolog-scryer
cargo build --release
./target/release/ape --text "Every man owns a car." --solo drspp
./target/release/ape --text "A man waits." --c cdrspp --c cparaphrase      # multi-mode XML
./target/release/ape serve --addr 127.0.0.1:8000
curl 'http://127.0.0.1:8000/?text=John+waits.&solo=fol'
curl http://127.0.0.1:8000/v1/chat/completions -H 'Content-Type: application/json' \
  -d '{"model":"ape-owlfsspp","messages":[{"role":"user","content":"Every man is a human."}]}'
```

## Verifying parity yourself (no Python anywhere)

```bash
# 1. SWI-Prolog reference outputs (only step that needs swipl besides the converter)
swipl harness/export_tests.pl  /path/to/APE harness/tests_canon.pl
swipl harness/run_swi.pl       /path/to/APE harness/tests_canon.pl harness/swi_results.txt
swipl harness/outmodes_swi.pl  /path/to/APE harness/tests_canon.pl harness/om_swi.txt 10
# 2. Rust side: in-process Scryer, one engine per core
cargo run --release --bin ape-parity -- suite       # DRS =@= parity + acceptance MCC -> harness/parity_report.json
cargo run --release --bin ape-parity -- outmodes    # 22 output modes, exact + normalised
cargo run --release --bin ape-parity -- fixtures    # regenerate tests/fixtures_swi.json
cargo run --release --bin ape-parity -- cycles      # cyclic use_module edges (must be 0)
cargo test --release                                # 515 golden byte-parity outputs
```

## Licence
APE is LGPL-3.0; the converted Prolog tree is a derivative and stays LGPL-3.0. Scryer is BSD-3-Clause.

## Long-running servers: engine recycling

Scryer never reclaims retracted clauses. APE asserts and retracts its user lexicon (`ulextext`) on every request, so a long-lived machine grows by about 2 MB per request: measured 354 → 625 MB over 120 requests in `examples/memgrowth.rs`.

`Ape` therefore rebuilds its Scryer machine every `APE_RECYCLE` requests (default 150, `0` = never). With recycling, memory plateaus at about 510 MB. During a 45-minute AceWiki oracle run the server stayed under 0.9 GB; before, it grew to 2.7 GB.

## Scryer Prolog version

`Cargo.toml` uses the fork branch `dhaneshkk/scryer-prolog@fix/ape-rs-findings` (pinned in `Cargo.lock`). It contains the `read/1` end-of-file fix that APE's user-lexicon loading needs; see that branch's commits.
