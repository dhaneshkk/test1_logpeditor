//! cargo run --release --example bench -- tests/fixtures/config.json
use iogp_runtime::{fast::Kind, load_config, reference, Engine, Frame, Object};
use std::time::Instant;

fn frames(workers: usize, n: usize) -> Vec<Frame> {
    (0..n).map(|i| {
        let t = i as f64 / 10.0;
        let mut objects = vec![];
        let mut facts = vec![];
        for (z, zt) in ["height-zone", "confined-space", "restricted-area", "lifting-zone", "hot-work-area", "live-equipment-zone", "machine-guard-zone"].iter().enumerate() {
            objects.push(Object { id: format!("z{z}"), zone: Some(zt.to_string()), camera: Some("CAM-03".into()), ..Default::default() });
        }
        for w in 0..workers {
            objects.push(Object { id: format!("p{w}"), class: Some("person".into()), ..Default::default() });
            facts.push(("in".to_string(), format!("p{w}"), format!("z{}", (w + i / 50) % 7)));
            if (w + i / 30) % 3 != 0 { objects.push(Object { id: format!("h{w}"), class: Some("harness".into()), ..Default::default() }); facts.push(("wear".into(), format!("p{w}"), format!("h{w}"))); }
            if w % 4 == 0 { objects.push(Object { id: format!("ph{w}"), class: Some("phone".into()), ..Default::default() }); facts.push(("use".into(), format!("p{w}"), format!("ph{w}"))); }
        }
        objects.push(Object { id: "load".into(), class: Some("suspended_load".into()), ..Default::default() });
        facts.push(("under".into(), "p0".into(), "load".into()));
        Frame { t, objects, facts, camera: Some("CAM-03".into()) }
    }).collect()
}

fn time<F: FnMut(&Frame) -> usize>(fs: &[Frame], mut f: F) -> (f64, usize) {
    for x in fs.iter().take(200) { f(x); }
    let t0 = Instant::now();
    let mut n = 0;
    for x in fs { n += f(x); }
    (t0.elapsed().as_secs_f64() / fs.len() as f64 * 1e6, n)
}

fn main() {
    let path = std::env::args().nth(1).unwrap_or("tests/fixtures/config.json".into());
    let rs = load_config(&std::fs::read_to_string(path).unwrap()).unwrap();
    println!("{} rules; µs per frame (one core), events in brackets", rs.rules.len());
    println!("{:>8} {:>14} {:>14} {:>14} {:>8}", "workers", "reference", "fast (Frame)", "fast (builder)", "speed-up");
    for workers in [1usize, 5, 20, 50] {
        let fs = frames(workers, 3000);
        let mut a = reference::Engine::new(rs.rules.clone(), rs.grace_seconds, rs.cooldown_seconds);
        let (ta, na) = time(&fs, |f| a.step(f).len());
        let mut b = Engine::new(rs.rules.clone(), rs.grace_seconds, rs.cooldown_seconds);
        let (tb, nb) = time(&fs, |f| b.step(f).len());
        let mut c = Engine::new(rs.rules.clone(), rs.grace_seconds, rs.cooldown_seconds);
        let (tc, nc) = time(&fs, |f| {
            c.begin(f.t, f.camera.as_deref());
            for o in &f.objects {
                if let Some(x) = &o.class { c.object(&o.id, Kind::Class, x, o.camera.as_deref()); }
                if let Some(x) = &o.zone { c.object(&o.id, Kind::Zone, x, o.camera.as_deref()); }
            }
            for (r, x, y) in &f.facts { c.fact(r, x, y); }
            c.end().len()
        });
        println!("{workers:>8} {:>9.1} [{na:>4}] {:>9.1} [{nb:>4}] {:>9.1} [{nc:>4}] {:>7.1}×", ta, tb, tc, ta / tc);
    }
}
