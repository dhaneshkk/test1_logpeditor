# iogp-editor — IOGP Life-Saving Rule alerts in controlled English

End users write alert rules as plain sentences. The detection framework receives the same rules as
first-order logic plus a detector configuration.

```
If a worker is in a height-zone and does not wear a harness for at least 3 seconds
then VisionGuard raises a critical working-at-height-alert for the worker.
```

becomes

```
fof(lsr9_R1, axiom,
  ! [T, W, Z] :
    ( ( person(W) & height_zone(Z) & in(W, Z, T)
        & ! [S] : (within(S, T, 3) => ~ (? [H] : (harness(H) & wear(W, H, S)))) )
      => alert(working_at_height, critical, W, T) )).
```

It also becomes a JSON rule that lists the detectors, zones and signals it needs, plus a
plain-language readback that the safety officer signs off:

> VisionGuard raises a critical Working at Height alert (IOGP Life-Saving Rule 9) about the worker when:
> • the worker is in the height-zone • the worker is not seen wearing a harness for at least 3 seconds

## How it works

| Layer | What it does |
|---|---|
| **Catalog** (`data/vg360_catalog.json`) | The vocabulary: detector classes, zones drawn per camera, observed relations (wear, carry, hold, use, drive, have), positions (in, under, near, on), site signals (permit, isolation-certificate, gas-test), the 9 IOGP Life-Saving Rule alerts, severities, units, grace and cooldown. The ACE lexicon and the editor grammar are generated from it. |
| **Predictive editor** (`src/grammar.rs`) | A Codeco grammar over the catalog, run by acewiki-rs's chart parser. It offers only words that can still lead to a valid rule: after *wears* only detector classes, after *has* only signals, after *is* the positions. |
| **APE via ape-rs** | Every sentence is parsed by the real Attempto Parsing Engine, converted to Scryer Prolog and run in-process. Its DRS is the meaning. Its paraphrase is shown back to the user. |
| **Compiler** (`src/compile.rs`) | DRS → checked rule. It enforces the policy: one `If … then`; only the agent raises; a negated visual condition needs a duration (positive-evidence policy); the alert subject must be bound. It emits time-indexed TPTP FOL with `within(S,T,D)`. APE errors become end-user messages ("'workr' is not a word VisionGuard knows — did you mean: worker?"). |
| **Runtime** (`src/runtime.rs`) | The reference evaluator, after yappu-fsm. **Evidence smoothing:** a detected fact stays believed for *grace* seconds after it was last seen. Durations are then evaluated exactly over believed facts. An **AlertMachine** (IDLE → ACTIVE → COOLING) raises each (rule, subject) alert once, with a proof. `Engine::yappu_compatible()` reproduces yappu-fsm's timer semantics. |

## Auto-suggestion while typing

The editor is a single text box that suggests as you type:

* **Dropdown of valid next words**, filtered by the part of the word already typed and grouped (detected object, zone, named zone, camera, record, position, relation, number, severity, alert). It also completes multi-word terms such as "does not" and "at least".
* **Grey ghost completion**: the rest of the current word, plus any following words the grammar forces (e.g. "VisionGuard raises a"). **Tab** accepts it, **→** at the end accepts one word, **↑/↓ Enter** picks from the list, **Ctrl+Space** shows everything, **Esc** closes the list.
* **Spelling fixes:** an unknown word is underlined in red and the dropdown offers the closest valid words ("crtical" → critical). A word in the wrong place gets the words that fit there. Accepting a fix mid-sentence replaces only that word.
* **Ranking** comes from a token n-gram model trained on the templates, every saved rule version and valid compile attempts, each distinct sentence counted once. It is retrained on every save, so suggestions follow how your officers write. Every suggestion is checked against the editor grammar first.
* When the sentence is complete, the compile panel (readback, FOL, JSON, save/submit) appears below.

API: `POST /suggest {"text": "..."}` → `{options:[{w,g,fix?}], ghost, partial, partial_start, bad_end, confident, complete, error?, groups}`. Use it to add the same suggestions to other front ends.

## Inference: `iogp-runtime` (no parser, no Prolog)

Scryer/APE run only when someone writes or checks a rule. At inference time the detection pipeline links the separate **`iogp-runtime`** crate (serde only): it loads the exported `config.json` and evaluates frames, through a Rust API or a C ABI (`iogp-runtime/include/iogp_runtime.h`). Per frame on one core with 15 rules it takes **2.3 µs** for one worker, **15 µs** for 20 and **40 µs** for 50 (0.8 µs for a small frame via the C ABI). That is 5–16× faster than the reference engine, with identical events checked on 72,000 frames. See `iogp-runtime/README.md`.

## Rule review with clingo

clingo (answer set programming, through the [clingox](https://github.com/papilionem/clingox) crate) is built into the editor and is used **only while rules are written and approved**. It is never part of `iogp-runtime`, so it adds nothing to inference latency.

```bash
iogp review --file rules.ace                # example scenes, dead rules, implications, coverage
iogp lp --file rules.ace > rules.lp         # the rules as a clingo program
```

* **`/export/rules.lp`**: the approved rules as an ASP program over `frame/1`, `obj/3`, `cam/3` and `fact/4` (milliseconds; believed facts). The detection team can run it with plain `clingo`.
* **Review queue:** each pending rule gets the **smallest example scene that fires it**, a **"can never fire"** warning when no scene can satisfy it, and **overlap with live rules**: "whenever this rule fires, live rule WAH fires too — it adds no new alert situations".
* **`/analysis`:** for the live rule set, Life-Saving Rules and named zones with no rule, dead rules, rules implied by other rules, and an example scene per rule.
* **Bounded:** the checks consider scenes containing the objects the rules mention (counts included), over the longest duration involved.
* **Trusted because it is tested:** `tests/asp_differential.rs` checks that clingo derives exactly what the reference engine derives, frame by frame, on noisy multi-camera scenes (6,317 identical derivations). Lean §10 proves that the run-start encoding of "for at least D" computes the start of the current streak, which is what the runtime's timer measures.

## Language extensions

| Need | Write | Compiles to |
|---|---|---|
| a specific zone | `If a worker is in Rig-Floor-East …` (named zones come from the catalog's `zone_instances`) | `height_zone(Z) & zone_id(Z, 'Rig-Floor-East')` |
| one camera | `… is in a height-zone of CAM-03 …` (cameras from the catalog's `cameras`) | `height_zone(Z) & camera_of(Z, 'CAM-03')` |
| permit exceptions | `… raises a high work-authorisation-alert for the worker unless Moonpool has a permit or the worker has a permit.` | an extra conjunct `~ (? [R] : (permit(R) & have(Z, R, T)))` per exception, marked `"exception": true` |

Before APE sees the sentence, "unless" is rewritten into the if-part ("and Moonpool does not have a permit"), so APE still checks it. Only records (permits, isolation certificates, gas tests) can be exceptions. "unless the worker wears a harness" is refused: an absence seen by a camera must be timed ("does not wear a harness for at least 3 seconds"). The exception must also name something the if-part already mentions.

## Approval workflow, versions, audit

* **Users:** `iogp user add NAME --role author|approver|admin --password …` creates `users.json` with salted SHA-256 password hashes; start the server with `serve --users users.json`. Without a users file the editor runs a flagged **demo mode** with three password-less identities.
* **Versions:** every save creates an immutable version: Draft → Submitted → **Approved** | Rejected. A new draft supersedes earlier drafts and pending submissions, and the live version stays in force until its replacement is approved. Retiring a rule takes it out of force.
* **Four-eyes:** approve/reject needs the approver role and a different person from the author. The store enforces it, not just the UI. A rejection needs a note, which the author sees.
* **Exports contain approved versions only.** `config.json` gives each rule's version, author, approver and approval time, plus a `revision` hash of the deployed set.
* **Audit log** (`<rules>.audit.jsonl`): one JSON line per action, each linked to the previous one by SHA-256. Pages: `/review` (approval queue), `/rules/{id}` (version history + audit trail), `/audit` (verified on every view). Use `iogp audit verify FILE` offline. The server refuses to start on a broken log.

## Real-site replay and officer sentences

```bash
iogp_eval replay --site site.json --detections export.jsonl --store iogp_rules.json --labels officer_labels.jsonl
```
* **`site.json`**: per camera, the zone polygons (ids = named zones), an optional `px_per_metre`, a `class_map` from detector labels to catalog labels, and permit timelines.
* **`export.jsonl`**: one line per camera frame, `{t | ts, camera, detections: [{track_id, class, bbox}], facts?}`.

Facts are derived from geometry by the catalog's detector names, as documented in `src/ingest.rs`, or passed through when VisionGuard360 already computes them. Each camera gets its own engine. With **labels** (`{rule | "LSR9", start, end, camera?, subject?}`) the output adds incidents caught (recall), false alarms per hour, median alert latency and frame-level MCC. `tools/sample_vg360_log.py` writes a synthetic sample in this format.

**Collecting how officers write:**
* Every complete sentence a user compiles is appended to `<rules>.attempts.jsonl`: editor, paste, save and API.
* The save form has an "intent" field for what the rule should catch, in the author's own words.
* `iogp_eval corpus <rules>.attempts.jsonl --store iogp_rules.json` reports the acceptance rate per user, the most common problems, **unknown words (vocabulary gaps)**, and intent → rule pairs. Use it to extend the catalog and the acceptance set.

## Requirements

| | Version | Why |
|---|---|---|
| Rust | **1.98** (pinned in `rust-toolchain.toml`, applied to ape-rs, acewiki-rs, iogp-editor and iogp-runtime) | clingox requires ≥ 1.98 |
| CMake | ≥ 3.16 | clingox builds clingo 5.8 from source |
| C/C++ compiler, git | — | clingo; the Scryer Prolog fork is fetched with git |

`scripts/check-toolchain.sh` checks all of them. On Ubuntu or WSL:

```bash
sudo apt install build-essential cmake git
rustup toolchain install 1.98        # or let rustup install it on the first cargo command here
./scripts/check-toolchain.sh
```

The first build compiles Scryer and clingo, which takes a few minutes. `iogp-runtime` declares Rust 1.98 for consistency but still depends only on serde, so a detection pipeline linking it needs no CMake.

## Run

```bash
cargo build --release
iogp serve --addr 127.0.0.1:9191 --rules iogp_rules.json [--users users.json]   # editor UI
iogp user add dana --role approver --password '…'             # users file for sign-off
iogp audit verify iogp_rules.audit.jsonl                       # check the audit chain
iogp compile --file rules.ace --format json|tptp|readback      # batch / CI (exit 2 on invalid rules)
iogp lexicon                                                   # generated ACE lexicon
iogp --catalog my_site.json serve                              # your own detectors and zones
```

`APE_PROLOG_DIR` (default `../ape-rs/prolog-scryer`) points at the converted APE sources.

### Web UI
* `/`: the predictive editor, the 9 LSR templates, a live compile panel (readback, FOL, JSON, APE paraphrase, errors with hints) and the rule set (edit / delete).
* `/catalog`: the vocabulary.
* `/export/config.json`: the whole rule set for the detection framework (schema `iogp-alert-rules/v1`: timing, semantics, required detectors, zones and signals, rules with FOL).
* `/export/rules.p`: TPTP. `/export/rules.ace`: the sentences.
* `/grammar.json`: the generated editor grammar.

### OpenAI-compatible API
`POST /v1/chat/completions`. Send rule sentences as the user message, one per line; an optional `ID:` prefix sets the rule id.

| model | returns |
|---|---|
| `iogp-rule-compiler` | JSON: rule-set config + `tptp` + `errors` (with hints) |
| `iogp-rule-explain` | plain-language readback, or why a sentence is not a valid rule |
| `iogp-rule-check` | valid / invalid per sentence |

`GET /v1/models` lists the models. `stream: true` is supported.

`POST /v1/simulate` takes `{"frames": [{"t", "objects": [{"id","class"|"zone"|"signal"}], "facts": [[rel, a, b]]}], "rules": [...]?}` and returns the alert events with proofs. Without `rules`, the saved rule set is used. Use it to replay recorded detections against a configuration before deploying it.

## Guarantees and measurements

* **Lean 4** (`formal/IogpAlerts.lean`, core only, 35 theorems, checked with Lean 4.15 and 4.25):
  * evidence smoothing: observation ⇒ belief; grace 0 is exact; monotone in grace; no evidence ⇒ no belief
  * **negated durations are exact**: "not believed throughout [T−D, T]" ⇔ "not observed throughout [T−D−g, T]". So a single missed frame never fires a "does not wear" rule, and a real absence is never reported late.
  * positive durations bridge gaps ≤ grace
  * AlertMachine:
    * raises only from IDLE
    * at most one alert per continuous episode
    * no re-raise within the cooldown
  * the inner-scope FOL (∀S ∃H) is strictly weaker than the outer scope, so it tolerates tracker-ID switches
  * MCC identities
  * **audit log**: with injective hashing, the head hash commits to the whole history, so any edit, removal or reordering is detected
  * **four-eyes**: every approved, and therefore exported, version was approved by someone other than its author; every workflow step preserves this
  * **exceptions and scoping only suppress alerts**: "unless E" ≡ "and not E"; more exceptions mean fewer alerts; narrower candidate zones mean fewer bindings
  * **parity reference RNG** (`src/yappu_ref.rs`): the port of Python's `random()` stays below 1 and its rejection-sampled `choice` index is always in range
* **MCC**:
  * acceptance set (14 valid / 16 invalid sentences, including scoping and exceptions): 1.0
  * replay of the synthetic sample site (2 cameras, 3 labelled incidents): 3/3 caught, 0 false alarms, median latency 3.6 s, frame MCC 0.90
  * parity against yappu-fsm's own EventEngine (1800 frames, 3 rules; reference frames from `iogp_eval yappu-ref`, a Rust port of yappu's engine that reproduces the Python output byte for byte): 0.9998; the one differing frame is a float-rounding artefact in yappu (`2.9999999999999996 < 3`)
  * synthetic site: 6 workers, 9 LSR rules, 600 s at 10 fps; the ground truth is the ideal evaluator on noise-free detections; random dropped detections:

    | missed detections | iogp engine | yappu timers | per-frame |
    |---|---|---|---|
    | 0 % | 0.936 | 0.952 | 0.745 |
    | 10 % | 0.937 | 0.921 | 0.677 |
    | 25 % | 0.932 | 0.754 | 0.564 |
    | 40 % | 0.881 | 0.616 | 0.458 |

    The engine's score barely moves until 25 % missed detections. Its residual error is the deliberate
    grace lag (0.7 s at each episode edge), visible at 0 %. That lag is also why short-episode rules such as
    LSR8 score ≈ 0.76 at every noise level. Lower `grace_seconds` in the catalog for cleaner cameras.
* **Tests**:
  * `cargo test`: grammar coverage of every template and acceptance sentence, sort-restricted suggestions, Timer / AlertMachine units, compiler decisions and error messages against APE, end-to-end runtime timing
  * `iogp_eval parity|synthetic` for the MCC numbers above

## Layout
```
data/vg360_catalog.json   vocabulary + LSR templates      data/acceptance.ace   labelled sentences
src/catalog.rs            catalog, lexicon generation     src/grammar.rs        editor grammar
src/drs.rs                APE DRS term parser             src/compile.rs        DRS -> rule, readback, FOL
src/config.rs             JSON / TPTP export              src/runtime.rs        evaluator + AlertMachine
src/store.rs              versions, workflow, audit, users src/ingest.rs        raw detection logs -> frames
iogp-runtime/             inference crate: fast + reference engines, config loader, C ABI, differential tests
src/web.rs                UI + OpenAI-compatible API      src/bin/iogp.rs, iogp_eval.rs
src/suggest.rs            auto-suggestion engine          src/editor.js         auto-suggest text box
src/asp.rs                rules -> clingo program; review (example scenes, dead rules, implications, coverage)
src/yappu_ref.rs          yappu-fsm engine port (parity)  formal/IogpAlerts.lean
tools/sample_vg360_log.py synthetic raw site log + labels
```
