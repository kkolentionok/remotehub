//! Conflict resolution: merging two snapshots into one.
//!
//! ## Model: record-level last-write-wins (LWW) + tombstones
//! Records are keyed by `(kind, id)`. To merge two snapshots we take the
//! union of their records; where both sides have the same key, the winner
//! is the one whose [`RecordMeta`] has the greater `(rev, origin)` (see
//! [`RecordMeta::wins_over`]). A tombstone is just a record, so a delete
//! competes with an edit on the same total order: a delete made *after*
//! an edit wins (the entity stays deleted); an edit made *after* a delete
//! wins (the entity comes back, "undelete"). This is convergent — every
//! device that sees the same two snapshots computes the identical result,
//! regardless of merge order — because the order is total and
//! deterministic.
//!
//! ## Why record-level (for v1) and not field-level
//! Record-level LWW is simple, fast, and provably convergent. Its one
//! cost: two devices editing *different fields* of the *same* record
//! concurrently — the higher-rev record wins wholesale and the other
//! field-edit is lost. For RemoteHub's reality (a solo user across 2-3 of
//! their own devices, hosts edited rarely) this is an uncommon case. The
//! planned refinement is field-level LWW using the (currently empty)
//! `RecordMeta.field_revs` map; the snapshot format already reserves room
//! for it, so the upgrade is additive. See `docs/specs/sync.md`.
//!
//! ## Clocks
//! The merged snapshot is stamped with the local node and a `generated`
//! clock that is the max of both inputs, so it sorts after everything it
//! contains.

use std::collections::BTreeMap;

use crate::clock::{Hlc, NodeId};
use crate::model::{EntityKind, SyncRecord, SyncSnapshot};

/// Merge `remote` into `local`, producing a new snapshot stamped as
/// produced by `local_node`. Neither input is mutated.
///
/// The result contains, for each `(kind, id)` seen in either input, the
/// winning record under the LWW total order. Tombstones are preserved
/// (they must keep propagating until every replica has applied them;
/// pruning old tombstones is a separate, time-based concern — see the
/// spec's "Tombstone GC").
#[must_use]
pub fn merge(local: &SyncSnapshot, remote: &SyncSnapshot, local_node: NodeId) -> SyncSnapshot {
    let mut winners: BTreeMap<(EntityKind, String), SyncRecord> = BTreeMap::new();

    for rec in local.records.iter().chain(remote.records.iter()) {
        let key = (rec.kind, rec.id.clone());
        match winners.get(&key) {
            Some(existing) if !rec.meta.wins_over(&existing.meta) => {
                // existing stays (it wins or it's an exact dup)
            }
            _ => {
                winners.insert(key, rec.clone());
            }
        }
    }

    let generated = local.generated.max(remote.generated);
    let records = winners.into_values().collect();
    let mut merged = SyncSnapshot::new(local_node, generated, records);
    // The notes key is create-once, never edited, so there is no revision to
    // compare — first one to exist wins. When both sides somehow minted one,
    // take the smaller string: an arbitrary but *deterministic* rule, so two
    // devices that raced converge on the same choice instead of flip-flopping.
    merged.notes_key_b64 = match (&local.notes_key_b64, &remote.notes_key_b64) {
        (Some(a), Some(b)) => Some(a.min(b).clone()),
        (Some(a), None) => Some(a.clone()),
        (None, b) => b.clone(),
    };
    merged
}

/// Merge two snapshots that were both produced remotely (neither is the
/// caller's local state) — e.g. consolidating history. Identical logic to
/// [`merge`] but lets the caller name the producing node.
#[must_use]
pub fn merge_as(a: &SyncSnapshot, b: &SyncSnapshot, node: NodeId, generated: Hlc) -> SyncSnapshot {
    let mut merged = merge(a, b, node);
    merged.generated = generated.max(merged.generated);
    merged
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::EntityKind;
    use rh_core::{Host, Protocol};

    fn node(s: &str) -> NodeId {
        NodeId::new(s)
    }

    fn host_rec(id: &str, name: &str, rev: Hlc, origin: &str) -> SyncRecord {
        let mut h = Host::new(name, "1.2.3.4", Protocol::Ssh, Some(22));
        h.id = rh_core::HostId::from_raw(id);
        SyncRecord::host(&h, rev, node(origin)).unwrap()
    }

    fn snap(node_s: &str, gen: Hlc, recs: Vec<SyncRecord>) -> SyncSnapshot {
        SyncSnapshot::new(node(node_s), gen, recs)
    }

    #[test]
    fn disjoint_records_are_unioned() {
        let a = snap("A", Hlc::new(1, 0), vec![host_rec("01A", "a", Hlc::new(1, 0), "A")]);
        let b = snap("B", Hlc::new(1, 0), vec![host_rec("01B", "b", Hlc::new(1, 0), "B")]);
        let m = merge(&a, &b, node("A"));
        assert_eq!(m.live_count(EntityKind::Host), 2);
    }

    #[test]
    fn higher_rev_wins_on_same_id() {
        let older = host_rec("01X", "old-name", Hlc::new(5, 0), "A");
        let newer = host_rec("01X", "new-name", Hlc::new(9, 0), "B");
        let a = snap("A", Hlc::new(5, 0), vec![older]);
        let b = snap("B", Hlc::new(9, 0), vec![newer]);
        let m = merge(&a, &b, node("A"));
        assert_eq!(m.records.len(), 1);
        assert_eq!(m.records[0].as_host().unwrap().name, "new-name");
    }

    #[test]
    fn merge_is_commutative() {
        let r1 = host_rec("01X", "from-A", Hlc::new(7, 0), "A");
        let r2 = host_rec("01X", "from-B", Hlc::new(7, 1), "B");
        let a = snap("A", Hlc::new(7, 0), vec![r1.clone()]);
        let b = snap("B", Hlc::new(7, 1), vec![r2.clone()]);
        let ab = merge(&a, &b, node("A"));
        let ba = merge(&b, &a, node("A"));
        assert_eq!(ab.records[0].as_host().unwrap().name, ba.records[0].as_host().unwrap().name);
        // (7,1) beats (7,0) regardless of order.
        assert_eq!(ab.records[0].as_host().unwrap().name, "from-B");
    }

    #[test]
    fn exact_rev_tie_breaks_by_origin_deterministically() {
        let r_a = host_rec("01X", "origin-A", Hlc::new(7, 0), "AAA");
        let r_b = host_rec("01X", "origin-B", Hlc::new(7, 0), "BBB");
        let a = snap("AAA", Hlc::new(7, 0), vec![r_a]);
        let b = snap("BBB", Hlc::new(7, 0), vec![r_b]);
        let m = merge(&a, &b, node("AAA"));
        // "BBB" > "AAA" lexically -> origin-B wins, both orders.
        assert_eq!(m.records[0].as_host().unwrap().name, "origin-B");
        let m2 = merge(&b, &a, node("AAA"));
        assert_eq!(m2.records[0].as_host().unwrap().name, "origin-B");
    }

    #[test]
    fn later_delete_beats_earlier_edit() {
        let edit = host_rec("01X", "edited", Hlc::new(3, 0), "A");
        let del = SyncRecord::tombstone(EntityKind::Host, "01X", Hlc::new(8, 0), node("B"));
        let a = snap("A", Hlc::new(3, 0), vec![edit]);
        let b = snap("B", Hlc::new(8, 0), vec![del]);
        let m = merge(&a, &b, node("A"));
        assert_eq!(m.records.len(), 1);
        assert!(m.records[0].is_deleted());
        assert_eq!(m.live_count(EntityKind::Host), 0);
    }

    #[test]
    fn later_edit_beats_earlier_delete_undelete() {
        let del = SyncRecord::tombstone(EntityKind::Host, "01X", Hlc::new(3, 0), node("A"));
        let edit = host_rec("01X", "resurrected", Hlc::new(8, 0), "B");
        let a = snap("A", Hlc::new(3, 0), vec![del]);
        let b = snap("B", Hlc::new(8, 0), vec![edit]);
        let m = merge(&a, &b, node("A"));
        assert!(!m.records[0].is_deleted());
        assert_eq!(m.records[0].as_host().unwrap().name, "resurrected");
    }

    #[test]
    fn merge_is_idempotent() {
        let r = host_rec("01X", "x", Hlc::new(2, 0), "A");
        let a = snap("A", Hlc::new(2, 0), vec![r]);
        let once = merge(&a, &a, node("A"));
        let twice = merge(&once, &a, node("A"));
        assert_eq!(once.records.len(), 1);
        assert_eq!(twice.records.len(), 1);
    }

    #[test]
    fn generated_clock_is_max_of_inputs() {
        let a = snap("A", Hlc::new(5, 0), vec![]);
        let b = snap("B", Hlc::new(9, 3), vec![]);
        let m = merge(&a, &b, node("A"));
        assert_eq!(m.generated, Hlc::new(9, 3));
    }
}
