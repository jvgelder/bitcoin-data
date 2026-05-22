//! Log2 histogram for distribution analysis.

use serde::de::Error as DeError;
use serde::ser::SerializeStruct;
use serde::{Deserializer, Serializer};

/// Log2 histogram of u64 values. Bucket k covers [2^k, 2^(k+1)).
/// Bucket 0 = value 1; we use a separate `zeros` counter for value 0.
#[derive(Debug, Clone)]
pub struct Log2Hist {
    pub zeros: u64,
    /// 64 buckets covers the full u64 range.
    pub buckets: [u64; 64],
    pub sum: u128,
    pub max: u64,
}

impl Default for Log2Hist {
    fn default() -> Self {
        Self { zeros: 0, buckets: [0; 64], sum: 0, max: 0 }
    }
}

impl serde::Serialize for Log2Hist {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let mut state = serializer.serialize_struct("Log2Hist", 4)?;
        state.serialize_field("zeros", &self.zeros)?;
        state.serialize_field("buckets", &self.buckets.as_slice())?;
        state.serialize_field("sum", &self.sum)?;
        state.serialize_field("max", &self.max)?;
        state.end()
    }
}

impl<'de> serde::Deserialize<'de> for Log2Hist {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(serde::Deserialize)]
        struct Log2HistHelper {
            zeros: u64,
            buckets: Vec<u64>,
            sum: u128,
            max: u64,
        }

        let helper = <Log2HistHelper as serde::Deserialize>::deserialize(deserializer)?;

        if helper.buckets.len() != 64 {
            return Err(D::Error::invalid_length(
                helper.buckets.len(),
                &"exactly 64 histogram buckets",
            ));
        }

        let mut buckets = [0u64; 64];
        buckets.copy_from_slice(&helper.buckets);

        Ok(Self {
            zeros: helper.zeros,
            buckets,
            sum: helper.sum,
            max: helper.max,
        })
    }
}

impl Log2Hist {
    pub fn record(&mut self, v: u64) {
        self.sum += v as u128;
        if v > self.max { self.max = v; }
        if v == 0 { self.zeros += 1; return; }
        let k = 63 - v.leading_zeros() as usize;
        self.buckets[k] += 1;
    }

    pub fn count(&self) -> u64 {
        self.zeros + self.buckets.iter().sum::<u64>()
    }

    pub fn mean(&self) -> f64 {
        let n = self.count();
        if n == 0 { 0.0 } else { self.sum as f64 / n as f64 }
    }

    /// Percentile in the range 0.0..=1.0. Uses bucket upper-bound as the
    /// conservative estimate for that bucket (overestimates slightly).
    pub fn percentile(&self, p: f64) -> u64 {
        let n = self.count();
        if n == 0 { return 0; }
        let target = (p * n as f64).ceil() as u64;
        let mut acc = self.zeros;
        if acc >= target { return 0; }
        for (k, &c) in self.buckets.iter().enumerate() {
            acc += c;
            if acc >= target {
                return (1u64 << k).saturating_mul(2).saturating_sub(1);
            }
        }
        self.max
    }

    pub fn merge(&mut self, other: &Log2Hist) {
        self.zeros += other.zeros;
        for i in 0..64 { self.buckets[i] += other.buckets[i]; }
        self.sum += other.sum;
        if other.max > self.max { self.max = other.max; }
    }

    pub fn print(&self, label: &str) {
        let n = self.count();
        if n == 0 { println!("{label}: (empty)"); return; }
        println!("{label}: n={} mean={:.1} max={} p50={} p90={} p99={} p99.9={} p99.99={}",
                 n, self.mean(), self.max,
                 self.percentile(0.50), self.percentile(0.90),
                 self.percentile(0.99), self.percentile(0.999), self.percentile(0.9999));
        println!("  log2 histogram (bucket k = offsets in [2^k, 2^(k+1))):");
        if self.zeros > 0 {
            println!("    0        : {:>10}  ({:.2}%)", self.zeros,
                     100.0 * self.zeros as f64 / n as f64);
        }
        for (k, &c) in self.buckets.iter().enumerate() {
            if c == 0 { continue; }
            let lo = 1u64 << k;
            let hi = lo.saturating_mul(2);
            println!("    2^{:<2} ({:>10}..{:<10}): {:>10}  ({:.2}%)",
                     k, lo, hi, c, 100.0 * c as f64 / n as f64);
        }
    }
}