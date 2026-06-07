//! Fanout combinator for `StatsSink`.

use crate::sinks::{DynStatsSink, StatsSink};
use crate::{BlockStats, Stats};
use async_trait::async_trait;

pub struct Fanout {
    sinks: Vec<DynStatsSink>,
    name: String,
}

impl Fanout {
    pub fn new(sinks: Vec<DynStatsSink>) -> Self {
        let name = format!("fanout({})", sinks.len());
        Self { sinks, name }
    }

    pub fn push(&mut self, sink: DynStatsSink) {
        self.sinks.push(sink);
        self.name = format!("fanout({})", self.sinks.len());
    }

    pub fn len(&self) -> usize {
        self.sinks.len()
    }
    pub fn is_empty(&self) -> bool {
        self.sinks.is_empty()
    }
}

#[async_trait]
impl StatsSink for Fanout {
    async fn emit_block(&self, row: &BlockStats) -> anyhow::Result<()> {
        if self.sinks.is_empty() {
            return Ok(());
        }
        let futs = self.sinks.iter().map(|s| {
            let s = s.clone();
            let row = row.clone();
            async move { (s.name().to_string(), s.emit_block(&row).await) }
        });
        collect(futures::future::join_all(futs).await, "emit_block")
    }

    async fn emit_run(&self, stats: &Stats) -> anyhow::Result<()> {
        if self.sinks.is_empty() {
            return Ok(());
        }
        let futs = self.sinks.iter().map(|s| {
            let s = s.clone();
            // Stats is not Clone — borrow lifetime via Arc would complicate
            // the API; instead serialize sequentially, which is fine since
            // emit_run runs once per scan.
            async move { (s.name().to_string(), s.emit_run(stats).await) }
        });
        collect(futures::future::join_all(futs).await, "emit_run")
    }

    async fn rollback_to_height(&self, height: u64) -> anyhow::Result<()> {
        if self.sinks.is_empty() {
            return Ok(());
        }
        let futs = self.sinks.iter().map(|s| {
            let s = s.clone();
            async move { (s.name().to_string(), s.rollback_to_height(height).await) }
        });
        collect(futures::future::join_all(futs).await, "rollback_to_height")
    }

    async fn flush(&self) -> anyhow::Result<()> {
        if self.sinks.is_empty() {
            return Ok(());
        }
        let futs = self.sinks.iter().map(|s| {
            let s = s.clone();
            async move { (s.name().to_string(), s.flush().await) }
        });
        collect(futures::future::join_all(futs).await, "flush")
    }

    fn name(&self) -> &str {
        &self.name
    }
}

fn collect(results: Vec<(String, anyhow::Result<()>)>, op: &str) -> anyhow::Result<()> {
    let errs: Vec<_> = results
        .into_iter()
        .filter_map(|(n, r)| r.err().map(|e| (n, e)))
        .collect();
    if errs.is_empty() {
        return Ok(());
    }
    let mut msg = format!("fanout {}: {} failed", op, errs.len());
    for (n, e) in &errs {
        msg.push_str(&format!("\n  [{n}] {e}"));
    }
    Err(anyhow::anyhow!(msg))
}
