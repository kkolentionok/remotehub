//! Integration tests for the storage layer.
//!
//! These run against an in-memory SQLite + [`MemoryKeychain`], so they
//! don't touch the user's real OS keychain and parallelize cleanly.

use std::sync::Arc;

use rh_core::settings::keys;
use rh_core::{
    Credential, CredentialKind, CredentialStore, EnvVar, GroupStore, Host, HostFilter, HostGroup,
    HostStore, Protocol, SecretValue, SettingsStore, Theme,
};
use rh_storage::{
    Db, Keychain, MemoryKeychain, SqliteCredentialStore, SqliteGroupStore, SqliteHostStore,
    SqliteSettingsStore,
};

/// Convenience: build a fully-wired set of stores with a fresh
/// in-memory DB and an in-memory keychain.
async fn setup() -> (
    SqliteHostStore,
    SqliteGroupStore,
    SqliteCredentialStore,
    SqliteSettingsStore,
    Arc<MemoryKeychain>,
) {
    let db = Db::open_memory().await.expect("open memory db");
    let keychain: Arc<MemoryKeychain> = Arc::new(MemoryKeychain::new());
    let kc_dyn: Arc<dyn Keychain> = keychain.clone();
    (
        SqliteHostStore::new(db.clone()),
        SqliteGroupStore::new(db.clone()),
        SqliteCredentialStore::new(db.clone(), kc_dyn),
        SqliteSettingsStore::new(db),
        keychain,
    )
}

// ====================================================================
// Hosts
// ====================================================================

#[tokio::test]
async fn host_create_get_roundtrip() {
    let (hosts, ..) = setup().await;
    let h = Host::new("prod-db-01", "db.example.com", Protocol::Ssh, None);

    hosts.create(&h).await.expect("create");
    let back = hosts.get(&h.id).await.expect("get");

    assert_eq!(back.name, "prod-db-01");
    assert_eq!(back.hostname, "db.example.com");
    assert_eq!(back.protocol, Protocol::Ssh);
    assert_eq!(back.port, 22);
}

#[tokio::test]
async fn host_create_preserves_tags_color_notes() {
    let (hosts, ..) = setup().await;
    let mut h = Host::new("test", "x.example.com", Protocol::Rdp, Some(13389));
    h.tags = vec!["prod".into(), "europe".into()];
    h.color = Some("#ff0000".into());
    h.notes = Some("# Important\n\nThis is the main server.".into());

    hosts.create(&h).await.unwrap();
    let back = hosts.get(&h.id).await.unwrap();

    assert_eq!(back.tags, vec!["prod", "europe"]);
    assert_eq!(back.color, Some("#ff0000".into()));
    assert!(back.notes.unwrap().contains("Important"));
}

#[tokio::test]
async fn host_create_preserves_stage18_fields() {
    let (hosts, ..) = setup().await;
    let mut h = Host::new("prod", "db.example.com", Protocol::Ssh, None);
    h.display_name = Some("Prod DB".into());
    h.startup_command = Some("tmux attach || tmux new".into());
    h.env_vars = vec![
        EnvVar::new("LANG", "en_US.UTF-8"),
        EnvVar::new("EDITOR", "vim"),
    ];
    h.detected_os = Some("ubuntu".into());

    hosts.create(&h).await.unwrap();
    let back = hosts.get(&h.id).await.unwrap();

    assert_eq!(back.display_name, Some("Prod DB".into()));
    assert_eq!(back.startup_command, Some("tmux attach || tmux new".into()));
    assert_eq!(back.env_vars, h.env_vars);
    assert_eq!(back.detected_os, Some("ubuntu".into()));
}

#[tokio::test]
async fn host_stage18_fields_default_empty_and_survive_update() {
    let (hosts, ..) = setup().await;
    // A bare host has no display_name / startup_command / detected_os
    // and an empty env_vars list.
    let h = Host::new("bare", "bare.example.com", Protocol::Ssh, None);
    hosts.create(&h).await.unwrap();
    let back = hosts.get(&h.id).await.unwrap();
    assert!(back.display_name.is_none());
    assert!(back.startup_command.is_none());
    assert!(back.detected_os.is_none());
    assert!(back.env_vars.is_empty());

    // Set them, then clear display_name/startup_command back to None.
    let mut edited = back;
    edited.display_name = Some("Bare".into());
    edited.startup_command = Some("htop".into());
    edited.env_vars = vec![EnvVar::new("FOO", "bar")];
    hosts.update(&edited).await.unwrap();
    let after_set = hosts.get(&edited.id).await.unwrap();
    assert_eq!(after_set.display_name, Some("Bare".into()));
    assert_eq!(after_set.env_vars, vec![EnvVar::new("FOO", "bar")]);

    let mut cleared = after_set;
    cleared.display_name = None;
    cleared.startup_command = None;
    cleared.env_vars = Vec::new();
    hosts.update(&cleared).await.unwrap();
    let after_clear = hosts.get(&cleared.id).await.unwrap();
    assert!(after_clear.display_name.is_none());
    assert!(after_clear.startup_command.is_none());
    assert!(after_clear.env_vars.is_empty());
}

#[tokio::test]
async fn host_list_filters_by_protocol() {
    let (hosts, ..) = setup().await;
    hosts
        .create(&Host::new("a", "a.example.com", Protocol::Ssh, None))
        .await
        .unwrap();
    hosts
        .create(&Host::new("b", "b.example.com", Protocol::Rdp, None))
        .await
        .unwrap();

    let ssh_only = hosts
        .list(HostFilter {
            protocol: Some(Protocol::Ssh),
            ..Default::default()
        })
        .await
        .unwrap();

    assert_eq!(ssh_only.len(), 1);
    assert_eq!(ssh_only[0].name, "a");
}

#[tokio::test]
async fn host_list_filters_by_search() {
    let (hosts, ..) = setup().await;
    hosts
        .create(&Host::new("alpha", "alpha.example.com", Protocol::Ssh, None))
        .await
        .unwrap();
    let mut beta = Host::new("beta", "beta.example.com", Protocol::Ssh, None);
    beta.tags = vec!["alpha-zone".into()];
    hosts.create(&beta).await.unwrap();
    hosts
        .create(&Host::new("gamma", "gamma.example.com", Protocol::Ssh, None))
        .await
        .unwrap();

    let matching = hosts
        .list(HostFilter {
            search: Some("alpha".into()),
            ..Default::default()
        })
        .await
        .unwrap();

    // "alpha" host (name), "beta" host (tag contains alpha-zone) → 2 matches.
    assert_eq!(matching.len(), 2);
}

#[tokio::test]
async fn host_update_changes_fields_and_advances_updated_at() {
    let (hosts, ..) = setup().await;
    let mut h = Host::new("original", "old.example.com", Protocol::Ssh, None);
    hosts.create(&h).await.unwrap();

    let before = h.updated_at;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    h.name = "renamed".into();
    h.hostname = "new.example.com".into();
    h.touch();

    hosts.update(&h).await.unwrap();
    let back = hosts.get(&h.id).await.unwrap();

    assert_eq!(back.name, "renamed");
    assert_eq!(back.hostname, "new.example.com");
    assert!(back.updated_at > before);
}

#[tokio::test]
async fn host_delete_removes_row() {
    let (hosts, ..) = setup().await;
    let h = Host::new("doomed", "x.example.com", Protocol::Ssh, None);
    hosts.create(&h).await.unwrap();

    hosts.delete(&h.id).await.unwrap();
    assert!(hosts.get(&h.id).await.is_err());
}

#[tokio::test]
async fn host_delete_unknown_id_errors() {
    let (hosts, ..) = setup().await;
    let fake = rh_core::HostId::new();
    let err = hosts.delete(&fake).await.unwrap_err();
    assert!(format!("{err}").contains("not found"));
}

// ====================================================================
// Groups
// ====================================================================

#[tokio::test]
async fn group_create_and_list_returns_tree_order() {
    let (_, groups, ..) = setup().await;
    let parent = HostGroup::new("Servers", None);
    let child = HostGroup::new("Prod", Some(parent.id.clone()));

    groups.create(&parent).await.unwrap();
    groups.create(&child).await.unwrap();

    let all = groups.list().await.unwrap();
    assert_eq!(all.len(), 2);
    // ordering puts NULL parent first
    assert_eq!(all[0].name, "Servers");
    assert_eq!(all[1].name, "Prod");
}

#[tokio::test]
async fn group_cycle_detection_self_parent() {
    let (_, groups, ..) = setup().await;
    let g = HostGroup::new("g", None);
    groups.create(&g).await.unwrap();

    let err = groups.move_to(&g.id, Some(&g.id)).await.unwrap_err();
    assert!(format!("{err}").contains("own parent"));
}

#[tokio::test]
async fn group_cycle_detection_indirect() {
    let (_, groups, ..) = setup().await;
    let a = HostGroup::new("A", None);
    let b = HostGroup::new("B", Some(a.id.clone()));
    let c = HostGroup::new("C", Some(b.id.clone()));
    groups.create(&a).await.unwrap();
    groups.create(&b).await.unwrap();
    groups.create(&c).await.unwrap();

    // A under C would create cycle: A → C → B → A.
    let err = groups.move_to(&a.id, Some(&c.id)).await.unwrap_err();
    assert!(format!("{err}").contains("cycle"));
}

#[tokio::test]
async fn group_delete_cascades_to_subgroups_and_orphans_hosts() {
    let (hosts, groups, ..) = setup().await;

    let parent = HostGroup::new("Servers", None);
    groups.create(&parent).await.unwrap();
    let child = HostGroup::new("Prod", Some(parent.id.clone()));
    groups.create(&child).await.unwrap();

    let mut h = Host::new("h1", "h1.example.com", Protocol::Ssh, None);
    h.group_id = Some(child.id.clone());
    hosts.create(&h).await.unwrap();

    // Delete parent — should cascade child group, and host should
    // survive but be orphaned (group_id NULL).
    groups.delete(&parent.id).await.unwrap();

    assert!(groups.get(&parent.id).await.is_err());
    assert!(groups.get(&child.id).await.is_err());

    let h_back = hosts.get(&h.id).await.unwrap();
    assert_eq!(h_back.group_id, None);
}

#[tokio::test]
async fn group_rename_works() {
    let (_, groups, ..) = setup().await;
    let g = HostGroup::new("old", None);
    groups.create(&g).await.unwrap();
    groups.rename(&g.id, "new").await.unwrap();
    assert_eq!(groups.get(&g.id).await.unwrap().name, "new");
}

// ====================================================================
// Credentials
// ====================================================================

#[tokio::test]
async fn credential_create_stores_secret_in_keychain_and_metadata_in_db() {
    let (_, _, creds, _, keychain) = setup().await;

    let cred = Credential::new("test-pwd", CredentialKind::Password, "alice");
    let secret = SecretValue::new(b"hunter2".to_vec());

    creds.create(&cred, secret, None).await.unwrap();

    // DB has the metadata
    let back = creds.get(&cred.id).await.unwrap();
    assert_eq!(back.name, "test-pwd");
    assert_eq!(back.username, "alice");
    assert_eq!(back.kind, CredentialKind::Password);

    // Keychain has the secret
    assert_eq!(keychain.entry_count(), 1);

    // Reveal returns the original secret
    let revealed = creds.reveal(&cred.id).await.unwrap();
    assert_eq!(revealed.expose(), b"hunter2");
}

#[tokio::test]
async fn credential_create_with_passphrase_uses_two_keychain_entries() {
    let (_, _, creds, _, keychain) = setup().await;

    let cred = Credential::new("test-key", CredentialKind::SshKey, "alice");
    let key = SecretValue::new(b"-----BEGIN OPENSSH PRIVATE KEY-----\n...".to_vec());
    let pp = SecretValue::new(b"secret-pp".to_vec());

    creds.create(&cred, key, Some(pp)).await.unwrap();

    assert_eq!(keychain.entry_count(), 2);

    let revealed_pp = creds.reveal_passphrase(&cred.id).await.unwrap().unwrap();
    assert_eq!(revealed_pp.expose(), b"secret-pp");
}

#[tokio::test]
async fn credential_reveal_passphrase_returns_none_for_password_kind() {
    let (_, _, creds, ..) = setup().await;

    let cred = Credential::new("pwd-cred", CredentialKind::Password, "alice");
    creds
        .create(&cred, SecretValue::new(b"x".to_vec()), None)
        .await
        .unwrap();

    let pp = creds.reveal_passphrase(&cred.id).await.unwrap();
    assert!(pp.is_none(), "password credentials have no passphrase concept");
}

#[tokio::test]
async fn credential_delete_removes_db_row_and_keychain_entries() {
    let (_, _, creds, _, keychain) = setup().await;

    let cred = Credential::new("doomed", CredentialKind::SshKey, "alice");
    creds
        .create(
            &cred,
            SecretValue::new(b"key".to_vec()),
            Some(SecretValue::new(b"pp".to_vec())),
        )
        .await
        .unwrap();
    assert_eq!(keychain.entry_count(), 2);

    creds.delete(&cred.id).await.unwrap();

    assert!(creds.get(&cred.id).await.is_err());
    assert_eq!(keychain.entry_count(), 0);
}

#[tokio::test]
async fn credential_rotate_secret_changes_keychain_but_keeps_metadata() {
    let (_, _, creds, ..) = setup().await;

    let cred = Credential::new("rotate-me", CredentialKind::Password, "alice");
    creds
        .create(&cred, SecretValue::new(b"old".to_vec()), None)
        .await
        .unwrap();

    creds
        .rotate_secret(&cred.id, SecretValue::new(b"new".to_vec()), None)
        .await
        .unwrap();

    // Metadata still present
    let back = creds.get(&cred.id).await.unwrap();
    assert_eq!(back.name, "rotate-me");

    // Secret is the new one
    let revealed = creds.reveal(&cred.id).await.unwrap();
    assert_eq!(revealed.expose(), b"new");
}

#[tokio::test]
async fn credential_unique_name_constraint() {
    let (_, _, creds, ..) = setup().await;

    let c1 = Credential::new("dup", CredentialKind::Password, "alice");
    creds
        .create(&c1, SecretValue::new(b"x".to_vec()), None)
        .await
        .unwrap();

    let c2 = Credential::new("dup", CredentialKind::Password, "bob");
    let err = creds
        .create(&c2, SecretValue::new(b"y".to_vec()), None)
        .await
        .unwrap_err();

    assert!(format!("{err}").contains("conflict") || format!("{err}").contains("UNIQUE"));
}

#[tokio::test]
async fn credential_unique_violation_cleans_up_keychain() {
    // Important: on DB failure during create, we MUST clean up the
    // keychain entry we wrote in step 1. Otherwise users get orphans.
    let (_, _, creds, _, keychain) = setup().await;

    let c1 = Credential::new("dup", CredentialKind::Password, "alice");
    creds
        .create(&c1, SecretValue::new(b"x".to_vec()), None)
        .await
        .unwrap();
    assert_eq!(keychain.entry_count(), 1);

    let c2 = Credential::new("dup", CredentialKind::Password, "bob"); // same name
    let _err = creds
        .create(&c2, SecretValue::new(b"y".to_vec()), None)
        .await
        .unwrap_err();

    // c2's keychain entry should have been cleaned up. c1's stays.
    assert_eq!(
        keychain.entry_count(),
        1,
        "failed create must not leave orphan keychain entries"
    );
}

#[tokio::test]
async fn credential_link_and_default_propagates_to_host_row() {
    let (hosts, _, creds, ..) = setup().await;

    let host = Host::new("h", "h.example.com", Protocol::Ssh, None);
    hosts.create(&host).await.unwrap();

    let cred = Credential::new("c", CredentialKind::Password, "alice");
    creds
        .create(&cred, SecretValue::new(b"x".to_vec()), None)
        .await
        .unwrap();

    creds.link_host(&host.id, &cred.id, true).await.unwrap();

    let back = hosts.get(&host.id).await.unwrap();
    assert_eq!(back.default_credential_id, Some(cred.id.clone()));
}

#[tokio::test]
async fn credential_link_only_one_default_per_host() {
    let (hosts, _, creds, ..) = setup().await;

    let host = Host::new("h", "h.example.com", Protocol::Ssh, None);
    hosts.create(&host).await.unwrap();

    let c1 = Credential::new("c1", CredentialKind::Password, "alice");
    let c2 = Credential::new("c2", CredentialKind::Password, "bob");
    creds
        .create(&c1, SecretValue::new(b"x".to_vec()), None)
        .await
        .unwrap();
    creds
        .create(&c2, SecretValue::new(b"y".to_vec()), None)
        .await
        .unwrap();

    creds.link_host(&host.id, &c1.id, true).await.unwrap();
    // Second link with set_as_default=true should usurp the default.
    creds.link_host(&host.id, &c2.id, true).await.unwrap();

    let back = hosts.get(&host.id).await.unwrap();
    assert_eq!(back.default_credential_id, Some(c2.id));
}

#[tokio::test]
async fn credential_unlink_default_clears_host_default() {
    let (hosts, _, creds, ..) = setup().await;

    let host = Host::new("h", "h.example.com", Protocol::Ssh, None);
    hosts.create(&host).await.unwrap();
    let cred = Credential::new("c", CredentialKind::Password, "alice");
    creds
        .create(&cred, SecretValue::new(b"x".to_vec()), None)
        .await
        .unwrap();

    creds.link_host(&host.id, &cred.id, true).await.unwrap();
    creds.unlink_host(&host.id, &cred.id).await.unwrap();

    let back = hosts.get(&host.id).await.unwrap();
    assert_eq!(back.default_credential_id, None);
}

// ====================================================================
// Settings
// ====================================================================

#[tokio::test]
async fn settings_load_returns_defaults_on_empty_db() {
    let (_, _, _, settings, _) = setup().await;
    let s = settings.load().await.unwrap();
    assert_eq!(s.theme, Theme::System);
    assert_eq!(s.terminal_font_size, 14);
}

#[tokio::test]
async fn settings_save_then_load_roundtrip() {
    let (_, _, _, settings, _) = setup().await;

    // The UI patches by Settings field name; storage maps these to the
    // namespaced table keys internally.
    let patch = serde_json::json!({
        "theme": "dark",
        "terminal_font_size": 16,
    });
    settings.save(patch).await.unwrap();

    let s = settings.load().await.unwrap();
    assert_eq!(s.theme, Theme::Dark);
    assert_eq!(s.terminal_font_size, 16);
    // Untouched keys should still have defaults
    assert_eq!(s.terminal_scrollback, 10_000);
}

#[tokio::test]
async fn settings_save_rejects_unknown_key() {
    let (_, _, _, settings, _) = setup().await;
    let patch = serde_json::json!({ "nonsense": "value" });
    let err = settings.save(patch).await.unwrap_err();
    assert!(format!("{err}").contains("unknown setting key"));
}

#[tokio::test]
async fn settings_save_rejects_non_object() {
    let (_, _, _, settings, _) = setup().await;
    let patch = serde_json::json!(["not", "an", "object"]);
    let err = settings.save(patch).await.unwrap_err();
    assert!(format!("{err}").contains("object"));
}

#[tokio::test]
async fn settings_load_returns_malformed_on_bad_json_value() {
    let (_, _, _, settings, _) = setup().await;
    // Save a valid value first to make sure load works.
    settings
        .save(serde_json::json!({ keys::THEME: "light" }))
        .await
        .unwrap();
    let s = settings.load().await.unwrap();
    assert_eq!(s.theme, Theme::Light);
}
