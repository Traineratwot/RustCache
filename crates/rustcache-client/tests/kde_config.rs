//! Dry-run / format tests for the KDE `kioslaverc` handler.
//!
//! `sysproxy::linux` exists only on Linux — this whole file is Linux-only.
#![cfg(target_os = "linux")]

use rustcache_client::sysproxy::handler::{envelope, handler_id, SysProxyHandler};
use rustcache_client::sysproxy::linux::kde::KdeHandler;

#[test]
fn dry_run_lists_kwrite_and_dbus() {
    let h = KdeHandler;
    let s = h
        .dry_run("127.0.0.1", 31280, &["example.com".into()])
        .unwrap();
    assert!(s.contains("kwriteconfig"), "expected kwriteconfig in:\n{s}");
    assert!(s.contains("--group 'Proxy Settings'"));
    assert!(s.contains("httpProxy '127.0.0.1 31280'"));
    assert!(s.contains("httpsProxy '127.0.0.1 31280'"));
    assert!(s.contains("ProxyType 1"));
    assert!(s.contains("NoProxyFor"));
    assert!(s.contains("example.com"));
    assert!(s.contains("dbus-send"));
}

#[test]
fn apply_message_wording_is_kde() {
    // We can't run apply against a real KDE session in CI, but the dry_run
    // path exercises the same value formatting. The apply() return string is
    // checked by inspection: "kde manual proxy {host}:{port}".
    // Here we assert the dry_run value format matches what apply() writes.
    let h = KdeHandler;
    let s = h.dry_run("127.0.0.1", 31280, &[]).unwrap();
    assert!(s.contains("'127.0.0.1 31280'"), "host:port space format");
}

#[test]
fn envelope_carries_handler_id() {
    let snap = serde_json::json!({"ProxyType": "1", "httpProxy": "127.0.0.1 31280"});
    let wrapped = envelope("kde", snap);
    assert_eq!(handler_id(&wrapped), Some("kde"));
    assert_eq!(wrapped["ProxyType"], "1");
}

#[test]
fn envelope_wraps_non_object() {
    let wrapped = envelope("gnome", serde_json::json!("raw"));
    assert_eq!(handler_id(&wrapped), Some("gnome"));
    assert_eq!(wrapped["value"], "raw");
}

#[test]
fn handler_id_missing() {
    assert_eq!(handler_id(&serde_json::json!({})), None);
}
