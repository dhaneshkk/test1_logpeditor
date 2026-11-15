//! Measures RSS growth of one embedded Scryer/APE engine over repeated queries.
fn rss_mb() -> f64 {
    std::fs::read_to_string("/proc/self/status").unwrap().lines().find(|l| l.starts_with("VmRSS")).unwrap()
        .split_whitespace().nth(1).unwrap().parse::<f64>().unwrap() / 1024.0
}
fn main() {
    std::thread::Builder::new().stack_size(256 << 20).spawn(|| {
        let mut ape = ape_rs::Ape::new("prolog-scryer").unwrap();
        println!("after load: {:.0} MB", rss_mb());
        for i in 1..=120 {
            let t = format!("Every country that borders a country{i} is a nation.");
            let lex = format!("noun_sg('country{i}', 'country{i}', undef). noun_sg('country', 'country', undef). noun_sg('nation', 'nation', undef). tv_finsg('borders', 'border'). ");
            let _ = ape.get_results(&[("text", &t), ("ulextext", &lex), ("uri", "http://x.org/geo"), ("noclex", "on"), ("guess", "off"), ("cparaphrase1", "on"), ("csyntaxpp", "on"), ("cowlxml", "on"), ("cowlfsspp", "on"), ("cdrspp", "on")]).unwrap();
            if i % 30 == 0 { println!("after {i} queries: {:.0} MB", rss_mb()); }
        }
    }).unwrap().join().unwrap();
}
