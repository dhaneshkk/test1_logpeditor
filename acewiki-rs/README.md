# acewiki-rs — AceWiki in Rust on ape-rs (Scryer Prolog)

> Toolchain: Rust **1.98**, pinned in `rust-toolchain.toml` (shared with iogp-editor; rustup installs it on first use).

A port of [AceWiki](https://github.com/AceWiki/AceWiki) (Java/Echo3, ≈45 k lines) to Rust. It
uses [ape-rs](../ape-rs): APE running inside Scryer Prolog, embedded in-process, with no
SWI-Prolog and no `ape.exe`.

| Java AceWiki                               | acewiki-rs                                                                                 |
|--------------------------------------------|--------------------------------------------------------------------------------------------|
| `ch.uzh.ifi.attempto.chartparser` (Codeco) | `src/chartparser`: 1:1 port of `ChartParser`, `FeatureMap`, `Category`, backrefs, scope-closing rules; grammar loaded from JSON generated from the same Codeco `.pl` sources (`grammar/generate_json.pl`) |
| `acewiki.aceowl.*` (ACE + OWL engine)      | `src/engine.rs`: `ACESentence.update` (APE multi-output call, OWL/OWLSWRL/reasonable flags, profile check), `ACEOWLLexicon`, questions, answers |
| `AceWikiOWLReasoner` + HermiT              | `src/owl/reasoner.rs`: native OWL 2 RL forward chainer and OWL 2 EL completion reasoner; `src/owl/owllink.rs`: OWLlink client for anything outside the native profile |
| `FileBasedStorage`                         | `src/storage.rs`: same on-disk format (directory form or `.acewikidata`), read and written |
| Echo3 GUI (`acewiki.gui`, `preditor`)      | `src/web`: axum + maud + HTMX. Article pages, predictive editor, lexicon editor, retract/reassert, conflict rejection, a→every suggestion, exports |
| `ch.uzh.ifi.attempto.aceeditor`            | `src/aceeditor.rs` + `/aceeditor`: ACE Editor app with its own grammar and lexicon file |
| —                                          | `/v1/chat/completions`: OpenAI-compatible endpoint (`acewiki-ask`, `acewiki-check`, `aceeditor-parse`) |

Out of scope: the GF multilingual module (`acewiki.gf`), as agreed.

## Run

```bash
cargo build --release
# wiki data: a directory of AceWiki data (e.g. AceWiki/docker/attempto/data), containing geo/ or geo.acewikidata
target/release/acewiki serve --data data --wiki geo --profile OWL2RL --addr 127.0.0.1:9090 \
    --prolog-dir ../ape-rs/prolog-scryer [--owllink http://127.0.0.1:8088/] [--read-only]
```

- Open http://127.0.0.1:9090/ for the wiki and http://127.0.0.1:9090/aceeditor for the ACE Editor.
- The first start analyses every sentence with APE: about 2 min for the geo wiki, ~860 sentences.
- Results are cached in `<data>/<wiki>.ape-cache.json`, so later starts take under 1 s.
- Wikis stored as `.acewikidata` are imported into directory form on first start.

```bash
curl -s localhost:9090/v1/chat/completions -H 'content-type: application/json' \
  -d '{"model":"acewiki-ask","messages":[{"role":"user","content":"Which country borders Germany?"}]}'
target/release/acewiki ask --data data --wiki geo "What is the capital of Germany?"
target/release/acewiki inspect --data data --wiki geo capital     # axioms + inferences for one word
```

### Reasoning

- **Native, in-process.**
  - Under the OWL 2 RL profile, a forward chainer with union-find equality, refutation for negated queries, and one prototype individual per class for subsumption.
  - Under the OWL 2 EL profile, completion rules CR1–CR6 and CR11, with nominals and range contexts.
  - The AceWiki `AceWikiOWLReasoner` contract is kept: flattened types and instances, strict super- and sub-classes, unique names via `DifferentIndividuals`, and questions answered as `SubClassOf(Q, Thing)`.
- **OWLlink fallback.** Used with `--reasoner owllink`, or automatically when the ontology contains axioms the native profile does not cover and `--owllink` is set.
  - `oracle/owllink/OwlLinkServer.java` is a dependency-free OWLlink server: JDK HttpServer, OWLAPI 3, and the HermiT jar from AceWiki's `lib/`.

## Parity with Java AceWiki

`oracle/Oracle.java` runs the real Java AceWiki headless: the same classes, HermiT, and APE via ape-rs's HTTP server. It dumps every observable decision to JSONL.

`acewiki-parity` recomputes the same decisions in Rust and scores them with exact agreement and **MCC**:

- sentence flags: OWL, OWL/SWRL, reasonable, integrated
- text and tokens
- inferred types, instances, super- and sub-concepts
- satisfiability
- question answers
- next-token options of the predictive editor

```bash
oracle/run_oracle.sh /tmp/geo geo OWL2RL http://127.0.0.1:8000/ oracle/out/geo_rl.jsonl reason
target/release/acewiki_parity --data data --wiki geo --profile OWL2RL --oracle oracle/out/geo_rl.jsonl --report report.json
```

### Results (Java AceWiki d1e758d + HermiT vs acewiki-rs)

The **inspected** column lists sentences / concepts / individuals / questions / editor prefixes.

| wiki     | profile | inspected                        | flags MCC | types | instances | super | sub | answers | editor options |
|----------|---------|----------------------------------|-----------|-------|-----------|-------|-----|---------|----------------|
| geo      | OWL 2 RL | 860 / 87 / 309 / 156 / 6 061     | 1.0       | 1.0   | 1.0       | 1.0   | 1.0 | 1.0     | 6 061 / 6 061 identical |
| geo      | OWL 2 EL | 860 / 87 / 309 / 156             | 1.0       | 1.0   | 1.0       | 1.0   | 1.0 | 1.0     | —              |
| attempto | OWL 2 RL | 978 / 102 / 371 / 102 / 7 273    | 1.0       | 1.0   | 1.0       | 1.0   | 1.0 | 1.0     | 7 273 / 7 273 identical |

**Notes on the counts.**
- Every number is the MCC over all (subject, candidate) pairs. For example, types MCC covers individual × concept, with tp=707 and tn=26 176 on geo RL. There are no false positives or false negatives anywhere.
- Sentence text, editor tokens, and the abstract and concrete next-token options are byte-identical, and so are completeness and back-references.
- Satisfiability is degenerate: no concept in either wiki is unsatisfiable. MCC is 1.0 there only because there are zero errors.

**Fixes found by the oracle.** Each now has a unit test.
- OWL 2 punning: the noun `capital` and the of-construct `capital of` share one IRI.
- Negative and universal ABox assertions must be re-checked to a fixpoint.
- Negation, and `≥n` under unique names, inside question classes.
- EL nominal rule for range contexts.
- Non-EL question classes under the EL profile.
- Java's editor tokenisation of definite proper names.

## Performance

### Predictive parser

These timings come from `examples/parser_bench.rs`. For every prefix of every sentence it computes what the editor shows: abstract and concrete next-word options, completeness and reference. The Java side is `Oracle … bench`, running the same loop on Java AceWiki's own `ChartParser` and lexicon. Machine: 2 vCPUs, best of 5 warm rounds.

| geo wiki, 6,061 prefixes, 1,221,776 options | time | per prefix |
|---|---|---|
| Java AceWiki d1e758d (JIT-warmed) | 8.1 s | 1.34 ms |
| Rust, straight port (before optimisation) | 17.4 s | 2.87 ms |
| **Rust, optimised** | **3.6 s** | **0.59 ms** |

The live server answers an editor request for a 9-word sentence in about 13 ms, HTML included.

All of the optimisations keep results exact; the parity check still finds all 13,334 option sets byte-identical to Java. Each change and the time after it:

1. No regex compilation in hot paths: `canonical_vars`, lexicon word tests, `use_an`. 17.4 → 10.8 s.
2. Edge identifiers are built into one buffer and cached per chart edge. Variable maps for copies are small linear maps, and chart sets use FxHash. → 6.8 s.
3. Per-category lexical rules are cached. Fulfilment of each abstract option is memoised by its canonical identifier; `formal/AceWikiMigration.lean` §7 proves memoisation exact. A cheap `similar_to` pre-filter runs before instantiating a rule. → 6.4 s.
4. The parser is cheap to clone. The engine keeps a prepared empty-prefix parser and forks it per request. Duplicate edges allocate nothing, and variable collection does not allocate. → 4.9 s.
5. Sorted merge-join for identifiers, plus mimalloc as the global allocator. → 3.6 s.

## Formal guarantees

`formal/AceWikiMigration.lean` (Lean 4, core only) proves the following:

- storage round-trip of comments
- the flag implications reasonable ⇒ OWL ⇒ OWL/SWRL
- soundness of load-time demotion
- the KB stays consistent under the commit discipline
- the engine-recycling bound
- the MCC identities used by the parity scores
- exactness of the parser's memoisation (§7)

## Scryer Prolog dependency

Both ape-rs and acewiki-rs depend on the fork branch
[`dhaneshkk/scryer-prolog@fix/ape-rs-findings`](https://github.com/dhaneshkk/scryer-prolog/tree/fix/ape-rs-findings),
pinned in `Cargo.lock`. That branch is upstream b7066b4 plus two commits:

- **`read/1` at end of stream.** It returns `end_of_file` when only layout (spaces, complete comments) follows the last clause. Upstream raised `syntax_error(incomplete_reduction)`, which AceWiki's user lexicon (ending in `". "`) triggered on every APE call. A regression test is included. ape-rs's former workaround, which trimmed the text in `open_string`, has been removed.
- **Unused-import warnings** in no-default-features builds are fixed.

Known Scryer limitations, still handled in ape-rs:

- Retracted clauses are never reclaimed, about 2 MB per APE request with a user lexicon. `Ape` rebuilds the machine every `APE_RECYCLE` requests (default 150). A real fix needs garbage collection of Scryer's code area.
- Loading the same module through a second path once panicked inside the embedded engine. It did not reproduce standalone. The converter emits canonical paths.

License: LGPL-3.0-or-later, as for AceWiki and APE. htmx (`static/htmx.min.js`) is 0BSD.
