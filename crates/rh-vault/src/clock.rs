//! Logical time for conflict resolution.
//!
//! Sync needs a way to order writes that happened on different devices,
//! possibly while offline, possibly with skewed wall clocks. A plain
//! wall-clock timestamp is unsafe: if device B's clock is 5 minutes
//! behind, an edit made *later* on B can lose to an *earlier* edit on A.
//!
//! We use a **Hybrid Logical Clock (HLC)**. An HLC tracks physical time
//! but never goes backwards and breaks within-millisecond ties with a
//! monotonic counter, so the order it produces is consistent with
//! causality as long as messages (snapshots) carry the clock forward.
//!
//! The [`Hlc`] is a `(wall_ms, counter)` pair with a total order. When a
//! device produces a new stamp it calls [`HlcGenerator::now`]; when it
//! receives a remote snapshot it folds the remote clock in via
//! [`HlcGenerator::observe`] so its own future stamps sort after anything
//! it has seen. Cross-device ties at the exact same `(wall_ms, counter)`
//! are broken one level up, by [`NodeId`] (see `merge.rs`).

use std::fmt;

use serde::{Deserialize, Serialize};

/// Stable per-device identifier, assigned once and persisted locally
/// (the `rh-app` layer stores it; a fresh ULID is a fine choice). Used
/// both as the deterministic tiebreaker in last-write-wins merges and as
/// the "who produced this" marker on snapshots and record metadata.
///
/// This is intentionally a thin newtype, not a ULID type from `rh-core`:
/// a node id is not a domain entity, it never indexes storage, and we
/// want it to round-trip any opaque string a future transport hands us.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct NodeId(String);

impl NodeId {
    #[must_use]
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for NodeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A Hybrid Logical Clock stamp.
///
/// Total order is `(wall_ms, counter)` lexicographically. `Ord` is
/// derived in exactly that field order, which is the whole point — do
/// not reorder the fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct Hlc {
    /// Physical time, milliseconds since the Unix epoch.
    pub wall_ms: u64,
    /// Logical counter, incremented for stamps that share a `wall_ms`.
    pub counter: u32,
}

impl Hlc {
    /// The zero stamp — sorts before every real stamp. Useful as the
    /// initial "rev" of a record that has never been written, or as the
    /// floor when observing remote clocks.
    pub const ZERO: Hlc = Hlc { wall_ms: 0, counter: 0 };

    #[must_use]
    pub fn new(wall_ms: u64, counter: u32) -> Self {
        Self { wall_ms, counter }
    }
}

impl fmt::Display for Hlc {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Compact, lexicographically-sortable-ish text form for logs.
        write!(f, "{}.{}", self.wall_ms, self.counter)
    }
}

/// Source of monotonic [`Hlc`] stamps for one device.
///
/// Not `Copy`/`Clone` of the *state* matters: hold one generator per
/// device (the `rh-app` layer keeps it in `AppState`). It is `Send` so it
/// can live behind a mutex.
#[derive(Debug)]
pub struct HlcGenerator {
    last: Hlc,
}

impl HlcGenerator {
    /// Start a generator. `seed` is the highest stamp this device has
    /// previously emitted or observed (persist it so restarts don't
    /// regress). Use [`Hlc::ZERO`] for a brand-new device.
    #[must_use]
    pub fn new(seed: Hlc) -> Self {
        Self { last: seed }
    }

    /// Current physical wall time in ms. Separated so tests can be
    /// deterministic by constructing the generator and driving it with
    /// [`HlcGenerator::tick`] instead.
    fn wall_now_ms() -> u64 {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0)
    }

    /// Produce the next stamp, using the real wall clock.
    pub fn now(&mut self) -> Hlc {
        self.tick(Self::wall_now_ms())
    }

    /// Produce the next stamp given an explicit physical time (testable).
    ///
    /// Rule: if physical time advanced past `last.wall_ms`, jump to it and
    /// reset the counter; otherwise keep `last.wall_ms` and bump the
    /// counter. Either way the result is strictly greater than `last`.
    pub fn tick(&mut self, physical_ms: u64) -> Hlc {
        let next = if physical_ms > self.last.wall_ms {
            Hlc::new(physical_ms, 0)
        } else {
            Hlc::new(self.last.wall_ms, self.last.counter.saturating_add(1))
        };
        self.last = next;
        next
    }

    /// Fold a remote stamp into this clock so future local stamps sort
    /// after it. Call this when importing a snapshot from another device.
    pub fn observe(&mut self, remote: Hlc) {
        if remote > self.last {
            self.last = remote;
        }
    }

    /// The highest stamp emitted/observed so far. Persist this as the
    /// next-boot seed.
    #[must_use]
    pub fn last(&self) -> Hlc {
        self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hlc_orders_by_wall_then_counter() {
        assert!(Hlc::new(10, 0) < Hlc::new(10, 1));
        assert!(Hlc::new(10, 9) < Hlc::new(11, 0));
        assert!(Hlc::ZERO < Hlc::new(1, 0));
    }

    #[test]
    fn tick_is_strictly_monotonic_even_with_stalled_clock() {
        let mut g = HlcGenerator::new(Hlc::ZERO);
        let a = g.tick(100);
        let b = g.tick(100); // same physical ms -> counter bumps
        let c = g.tick(100);
        assert!(a < b && b < c);
        assert_eq!(c, Hlc::new(100, 2));
    }

    #[test]
    fn tick_resets_counter_when_clock_advances() {
        let mut g = HlcGenerator::new(Hlc::ZERO);
        let _ = g.tick(100);
        let _ = g.tick(100);
        let advanced = g.tick(200);
        assert_eq!(advanced, Hlc::new(200, 0));
    }

    #[test]
    fn tick_never_regresses_when_clock_goes_backwards() {
        let mut g = HlcGenerator::new(Hlc::new(500, 3));
        // Wall clock jumped backwards (NTP correction, VM pause, etc.).
        let next = g.tick(100);
        assert!(next > Hlc::new(500, 3));
        assert_eq!(next, Hlc::new(500, 4));
    }

    #[test]
    fn observe_advances_past_remote() {
        let mut g = HlcGenerator::new(Hlc::new(100, 0));
        g.observe(Hlc::new(900, 5));
        let next = g.tick(100); // local wall is behind the observed remote
        assert!(next > Hlc::new(900, 5));
    }

    #[test]
    fn now_is_monotonic() {
        let mut g = HlcGenerator::new(Hlc::ZERO);
        let a = g.now();
        let b = g.now();
        assert!(b > a);
    }

    #[test]
    fn node_id_roundtrips_serde() {
        let n = NodeId::new("01J9ABCDEF");
        let j = serde_json::to_string(&n).unwrap();
        assert_eq!(j, "\"01J9ABCDEF\"");
        let back: NodeId = serde_json::from_str(&j).unwrap();
        assert_eq!(n, back);
    }
}
