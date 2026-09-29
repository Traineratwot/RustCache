//! System-proxy handler trait — one implementation per platform / DE.

use anyhow::Result;
use serde_json::Value;

/// Contract every platform handler implements.
///
/// The snapshot envelope always carries `"handler": id` so restore can pick
/// the right backend even if the desktop environment changed since apply.
pub trait SysProxyHandler {
    /// Stable identifier written into the snapshot envelope (`"kde"`, `"gnome"`, …).
    fn id(&self) -> &'static str;

    /// Capture current system-proxy settings.
    fn snapshot(&self) -> Result<Value>;

    /// Point system proxy at `host:port`, adding `bypass` to the no-proxy list.
    fn apply(&self, host: &str, port: u16, bypass: &[String]) -> Result<String>;

    /// Restore settings previously captured by [`Self::snapshot`].
    fn restore(&self, prev: &Value) -> Result<String>;

    /// One-line human-readable status.
    fn status(&self) -> Result<String>;

    /// Exact commands that [`Self::apply`] would run (for `--dry-run`).
    fn dry_run(&self, host: &str, port: u16, bypass: &[String]) -> Result<String>;

    /// Current effective HTTP proxy as `(host, port)`, if set.
    fn current_proxy(&self) -> Result<Option<(String, u16)>>;
}

/// Wrap a platform snapshot with the handler id so restore can dispatch.
pub fn envelope(handler_id: &str, snap: Value) -> Value {
    let mut obj = match snap {
        Value::Object(m) => m,
        other => {
            let mut m = serde_json::Map::new();
            m.insert("value".into(), other);
            m
        }
    };
    obj.insert("handler".into(), Value::String(handler_id.to_string()));
    Value::Object(obj)
}

/// Extract the handler id from a snapshot envelope (if present).
pub fn handler_id(snap: &Value) -> Option<&str> {
    snap.get("handler").and_then(|v| v.as_str())
}
