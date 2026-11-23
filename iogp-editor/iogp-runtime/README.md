# iogp-runtime — inference-time evaluation of IOGP alert rules

The editor (`iogp-editor`) writes and approves rules in controlled English, using APE and Scryer
Prolog, and exports them as `config.json`. **This crate is all a detection pipeline links at
inference time:** it loads that file and evaluates every camera frame. It contains no parser, no
Prolog and no solver; its only dependencies are `serde` and `serde_json`.

## Speed

Measured per frame on one core, 15 rules (the 9 Life-Saving Rule templates plus named-zone,
camera, permit-exception, count and nearness rules), with events identical to the reference:

| workers in view | reference engine | **fast engine** | speed-up |
|---|---|---|---|
| 1 | 15.4 µs | **2.3 µs** | 5× |
| 5 | 55.9 µs | **5.0 µs** | 12× |
| 20 | 210.8 µs | **15.1 µs** | 14× |
| 50 | 663.6 µs | **39.9 µs** | 16× |

Through the C ABI, a one-worker frame takes 0.8 µs. A detector network takes tens of
milliseconds per frame, so rule evaluation is a negligible share of the latency.

What makes it fast:
* object ids, types and relations are interned to `u32`;
* per-frame indexes are reused across frames, so a steady stream allocates almost nothing;
* bindings are enumerated with an odometer, not hash maps;
* duration timers are keyed by (rule, condition) and then by the binding slice, so lookups don't allocate;
* internal maps use an Fx hasher instead of SipHash;
* proof text is built only for alerts that are actually raised;
* unused symbols are released every 1024 frames, so ever-new track ids don't grow memory (tested with 20,000 tracks).

## Rust

```rust
let mut eng = iogp_runtime::Engine::from_config(&std::fs::read_to_string("config.json")?)?;
// either whole frames …
let events = eng.step(&frame);                     // Frame { t, camera, objects, facts }
// … or the zero-copy builder
eng.begin(t, Some("CAM-03"));
eng.object("7", Kind::Class, "person", None);
eng.object("Rig-Floor-East", Kind::Zone, "height-zone", None);
eng.fact("in", "7", "Rig-Floor-East");
for e in eng.end() { /* e.kind, e.rule, e.alert, e.lsr, e.severity, e.subject, e.t, e.proof */ }
```

## C / C++

`include/iogp_runtime.h`; link `libiogp_runtime.a` (or `.so`) with `-lpthread -ldl -lm`:

```c
iogp_engine *e = iogp_engine_new(config_json_text, &err);
iogp_frame_begin(e, t, "CAM-03");
iogp_frame_object(e, "7", IOGP_CLASS, "person", NULL);
iogp_frame_fact(e, "in", "7", "z1");
int n = iogp_frame_end(e);
for (int i = 0; i < n; i++) { iogp_event ev; iogp_event_get(e, i, &ev); }
```

`examples/c/build_and_run.sh` builds and runs `examples/c/demo.c`. Every entry point catches
panics and rejects NULL or invalid arguments with -1 or NULL, and `iogp_last_error` explains why.
Use one engine per camera stream.

## Correctness

`reference::Engine` is the straightforward evaluator that defines the semantics: evidence
smoothing (every type an object was seen with — class, zone, signal, or several classes on one id —
stays believed for `grace` seconds), duration timers, positive evidence, counts, scoping, exceptions, and an alert machine
that raises once and then cools down. `tests/differential.rs` runs both engines on 72,000 noisy
multi-camera frames in all three modes (smoothing, yappu-compatible, per-frame). It requires
identical events (kind, rule, subject, alert, LSR, severity, time, in order) and identical active
sets after every frame: 16,027 events so far. The Lean proofs in `../formal/IogpAlerts.lean`
describe the reference semantics.
