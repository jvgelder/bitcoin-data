//! Synthetic spend-delay distribution for quick benchmarks without a node.

use btc_data_stats::Stats;

pub fn run(num_outputs: u64, seed: u64) {
    let mut rng = Xorshift64::new(seed);
    let mut stats = Stats::new();
    for id in 1..=num_outputs {
        stats.outputs += 1;
        if sample_delay_ids(&mut rng).is_some() {
            let _ = id;
            stats.record_spend(btc_data_stats::SpendContext::Earlier);
        }
        if id % 200_000 == 0 { eprintln!("simulated {id}/{num_outputs}"); }
    }
    stats.print_report();
}

/// Spend-delay distribution in ID units. Returns None for ~5% "never spent".
/// Buckets mirror observed Bitcoin behaviour: 30% fast, 35% medium, etc.
fn sample_delay_ids(rng: &mut Xorshift64) -> Option<u64> {
    if rng.f64() < 0.05 { return None; }
    Some(match rng.f64() {
        p if p < 0.30 => rng.range(1, 100),
        p if p < 0.65 => rng.range(100, 3_000),
        p if p < 0.85 => rng.range(3_000, 50_000),
        p if p < 0.95 => rng.range(50_000, 500_000),
        _             => rng.range(500_000, 10_000_000),
    })
}

struct Xorshift64 { state: u64 }
impl Xorshift64 {
    fn new(seed: u64) -> Self { Self { state: seed ^ 0x9e37_79b9_7f4a_7c15 } }
    fn next(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13; x ^= x >> 7; x ^= x << 17;
        self.state = x; x
    }
    fn f64(&mut self) -> f64 { (self.next() >> 11) as f64 / (1u64 << 53) as f64 }
    fn range(&mut self, lo: u64, hi: u64) -> u64 { lo + self.next() % (hi - lo) }
}