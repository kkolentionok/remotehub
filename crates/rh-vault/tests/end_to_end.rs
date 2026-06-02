//! End-to-end: two devices converge through an encrypted remote blob.
//!
//! Simulates the full sync loop without any real backend:
//!   device builds snapshot -> seal -> push (opt. concurrency)
//!   other device pull -> open -> merge -> seal -> push
//! and asserts both devices converge to the same merged state, with the
//! credential secret surviving the round-trip and never appearing in
//! plaintext on the wire.

use rh_core::{Credential, CredentialKind, Host, HostId, Protocol};
use rh_vault::{
    from_export_string, merge, open_envelope, seal_snapshot_with, to_export_string,
    EntityKind, Hlc, HlcGenerator, KdfAlgo, KdfParams, MemoryRemote, NodeId, SyncCredentialPayload,
    SyncRecord, SyncRemote, SyncSnapshot,
};

const PW: &[u8] = b"shared-master-password";

// Cheap KDF so the test suite is fast (production uses 64 MiB defaults).
fn cheap_kdf() -> KdfParams {
    KdfParams {
        algo: KdfAlgo::Argon2id,
        m_cost_kib: 64,
        t_cost: 1,
        p_cost: 1,
        salt: rh_vault::gen_salt(),
    }
}

fn host(id: &str, name: &str) -> Host {
    let mut h = Host::new(name, "10.0.0.1", Protocol::Ssh, Some(22));
    h.id = HostId::from_raw(id);
    h
}

async fn seal_and_push(
    remote: &MemoryRemote,
    snap: &SyncSnapshot,
    expected: Option<&str>,
) -> String {
    let env = seal_snapshot_with(snap, PW, cheap_kdf()).unwrap();
    let bytes = to_export_string(&env).unwrap().into_bytes();
    remote.push(&bytes, expected).await.unwrap()
}

async fn pull_and_open(remote: &MemoryRemote) -> Option<(SyncSnapshot, String)> {
    let blob = remote.pull().await.unwrap()?;
    let env = from_export_string(std::str::from_utf8(&blob.bytes).unwrap()).unwrap();
    let snap = open_envelope(&env, PW).unwrap();
    Some((snap, blob.version))
}

#[tokio::test]
async fn two_devices_converge_through_remote() {
    let remote = MemoryRemote::new();

    // --- Device A: a host + a credential with a secret. ---
    let node_a = NodeId::new("device-A");
    let mut clk_a = HlcGenerator::new(Hlc::ZERO);
    let cred = Credential::new("root pw", CredentialKind::Password, "root");
    let cred_payload = SyncCredentialPayload {
        credential: cred.clone(),
        secret: Some(b"super-secret".to_vec()),
        passphrase: None,
    };
    let snap_a = SyncSnapshot::new(
        node_a.clone(),
        clk_a.now(),
        vec![
            SyncRecord::host(&host("01HOSTA", "web-A"), clk_a.now(), node_a.clone()).unwrap(),
            SyncRecord::credential(&cred_payload, clk_a.now(), node_a.clone()).unwrap(),
        ],
    );
    // First sync: remote empty -> expected None.
    let v1 = seal_and_push(&remote, &snap_a, None).await;

    // The blob on the wire must not contain the plaintext secret.
    let on_wire = remote.pull().await.unwrap().unwrap();
    assert!(!String::from_utf8_lossy(&on_wire.bytes).contains("super-secret"));

    // --- Device B: pulls, folds clock, adds its own host, merges, pushes. ---
    let node_b = NodeId::new("device-B");
    let (remote_snap, ver) = pull_and_open(&remote).await.unwrap();
    assert_eq!(ver, v1);
    let mut clk_b = HlcGenerator::new(Hlc::ZERO);
    clk_b.observe(remote_snap.generated);

    let local_b = SyncSnapshot::new(
        node_b.clone(),
        clk_b.now(),
        vec![SyncRecord::host(&host("01HOSTB", "web-B"), clk_b.now(), node_b.clone()).unwrap()],
    );
    let merged_b = merge(&local_b, &remote_snap, node_b.clone());
    let v2 = seal_and_push(&remote, &merged_b, Some(&ver)).await;
    assert_ne!(v1, v2);

    // --- Device A: pulls the merged state. ---
    let (final_a, _) = pull_and_open(&remote).await.unwrap();

    // Both hosts present, credential present with its secret intact.
    assert_eq!(final_a.live_count(EntityKind::Host), 2);
    assert_eq!(final_a.live_count(EntityKind::Credential), 1);
    let cred_rec = final_a
        .records
        .iter()
        .find(|r| r.kind == EntityKind::Credential)
        .unwrap();
    let recovered = cred_rec.as_credential().unwrap();
    assert_eq!(recovered.credential.name, "root pw");
    assert_eq!(recovered.secret.as_deref(), Some(&b"super-secret"[..]));
}

#[tokio::test]
async fn concurrent_push_forces_remerge() {
    let remote = MemoryRemote::new();
    let node_a = NodeId::new("A");
    let node_b = NodeId::new("B");
    let mut clk = HlcGenerator::new(Hlc::ZERO);

    // Seed the remote.
    let seed = SyncSnapshot::new(
        node_a.clone(),
        clk.now(),
        vec![SyncRecord::host(&host("01X", "seed"), clk.now(), node_a.clone()).unwrap()],
    );
    let v1 = seal_and_push(&remote, &seed, None).await;

    // Both devices pull v1.
    let (snap_for_a, ver_a) = pull_and_open(&remote).await.unwrap();
    let (snap_for_b, ver_b) = pull_and_open(&remote).await.unwrap();
    assert_eq!(ver_a, v1);
    assert_eq!(ver_b, v1);

    // A edits and pushes first (succeeds).
    let mut clk_a = HlcGenerator::new(snap_for_a.generated);
    let edit_a = SyncSnapshot::new(
        node_a.clone(),
        clk_a.now(),
        vec![SyncRecord::host(&host("01X", "edited-by-A"), clk_a.now(), node_a.clone()).unwrap()],
    );
    let merged_a = merge(&edit_a, &snap_for_a, node_a.clone());
    let _v2 = seal_and_push(&remote, &merged_a, Some(&ver_a)).await;

    // B tries to push against the now-stale v1 -> conflict.
    let mut clk_b = HlcGenerator::new(snap_for_b.generated);
    let edit_b = SyncSnapshot::new(
        node_b.clone(),
        clk_b.now(),
        vec![SyncRecord::host(&host("01X", "edited-by-B"), clk_b.now(), node_b.clone()).unwrap()],
    );
    let merged_b = merge(&edit_b, &snap_for_b, node_b.clone());
    let env_b = seal_snapshot_with(&merged_b, PW, cheap_kdf()).unwrap();
    let bytes_b = to_export_string(&env_b).unwrap().into_bytes();
    let conflict = remote.push(&bytes_b, Some(&ver_a)).await;
    assert!(matches!(conflict, Err(rh_vault::VaultError::RemoteConflict)));

    // B re-pulls, re-merges, retries -> succeeds, and B's later edit wins
    // (its HLC was generated after observing A's clock would be the real
    // flow; here B simply has the higher stamp).
    let (latest, latest_ver) = pull_and_open(&remote).await.unwrap();
    let remerged = merge(&merged_b, &latest, node_b.clone());
    let _v3 = seal_and_push(&remote, &remerged, Some(&latest_ver)).await;

    let (done, _) = pull_and_open(&remote).await.unwrap();
    assert_eq!(done.live_count(EntityKind::Host), 1);
}
