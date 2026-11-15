//! Per-frame cost of the inference path (compiled rules -> Rust engine). APE is used once, before timing.
use iogp_editor::{catalog::Catalog, runtime::{Engine, Frame, Object}, Compiler};
use std::time::Instant;
fn main() {
    std::thread::Builder::new().stack_size(256 << 20).spawn(|| {
        let cat = Catalog::builtin();
        let mut c = Compiler::new(cat.clone(), ape_rs::Ape::new("../ape-rs/prolog-scryer").unwrap());
        let rules: Vec<_> = cat.templates.iter().enumerate().map(|(i, t)| c.compile(&format!("R{i}"), &t.text).unwrap()).collect();
        for workers in [1usize, 5, 20] {
            // a busy frame: N workers, PPE, zones, a load, a vehicle, permits
            let frames: Vec<Frame> = (0..3000).map(|i| {
                let t = i as f64 / 10.0;
                let mut objects = vec![];
                let mut facts = vec![];
                for (z, zt) in ["height-zone", "confined-space", "restricted-area", "lifting-zone", "hot-work-area", "live-equipment-zone", "machine-guard-zone"].iter().enumerate() {
                    objects.push(Object { id: format!("z{z}"), zone: Some(zt.to_string()), ..Default::default() });
                }
                for w in 0..workers {
                    objects.push(Object { id: format!("p{w}"), class: Some("person".into()), ..Default::default() });
                    facts.push(("in".to_string(), format!("p{w}"), format!("z{}", (w + i / 50) % 7)));
                    if (w + i / 30) % 3 != 0 { objects.push(Object { id: format!("h{w}"), class: Some("harness".into()), ..Default::default() }); facts.push(("wear".into(), format!("p{w}"), format!("h{w}"))); }
                    if w % 4 == 0 { objects.push(Object { id: format!("ph{w}"), class: Some("phone".into()), ..Default::default() }); facts.push(("use".into(), format!("p{w}"), format!("ph{w}"))); }
                }
                objects.push(Object { id: "load".into(), class: Some("suspended_load".into()), ..Default::default() });
                facts.push(("under".into(), "p0".into(), "load".into()));
                Frame { t, objects, facts, camera: None }
            }).collect();
            let mut e = Engine::new(rules.clone(), 0.7, 3.0);
            for f in frames.iter().take(100) { e.step(f); } // warm-up
            let mut e = Engine::new(rules.clone(), 0.7, 3.0);
            let t0 = Instant::now();
            let mut n = 0;
            for f in &frames { n += e.step(f).len(); }
            let per = t0.elapsed().as_secs_f64() / frames.len() as f64;
            println!("{workers:>3} workers, 9 rules: {:>8.1} µs per frame  ({n} events over {} frames)", per * 1e6, frames.len());
        }
    }).unwrap().join().unwrap();
}
