//! Single-flight coalescing for cache misses on the same key.

use std::collections::HashMap;
use std::future::Future;

use parking_lot::Mutex;
use tokio::sync::broadcast;

/// Ensures only one upstream fetch runs per cache key at a time.
/// Concurrent waiters receive the same outcome.
pub struct Coalesce<T: Clone> {
    inflight: Mutex<HashMap<String, broadcast::Sender<T>>>,
}

impl<T: Clone> Default for Coalesce<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Clone> Coalesce<T> {
    pub fn new() -> Self {
        Self {
            inflight: Mutex::new(HashMap::new()),
        }
    }

    /// Run `f` for `key`, or wait for an in-flight run.
    ///
    /// On error the in-flight entry is removed and waiters retry via their own call.
    pub async fn run<F, Fut, E>(&self, key: &str, f: F) -> Result<T, E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<T, E>>,
    {
        let rx_opt = {
            let mut map = self.inflight.lock();
            if let Some(tx) = map.get(key) {
                Some(tx.subscribe())
            } else {
                let (tx, _rx) = broadcast::channel::<T>(1);
                map.insert(key.to_string(), tx.clone());
                None
            }
        };

        if let Some(mut rx) = rx_opt {
            loop {
                match rx.recv().await {
                    Ok(v) => return Ok(v),
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => {
                        // leader failed — run our own fetch
                        return f().await;
                    }
                }
            }
        }

        // Leader path.
        let tx = {
            let map = self.inflight.lock();
            map.get(key).cloned()
        };
        let result = f().await;
        {
            let mut map = self.inflight.lock();
            map.remove(key);
        }
        if let (Some(v), Some(tx)) = (result.as_ref().ok().cloned(), tx) {
            let _ = tx.send(v);
        }
        result
    }

    pub fn inflight_len(&self) -> usize {
        self.inflight.lock().len()
    }
}

/// Shared handle.
// Note: a `SharedCoalesce<T>` alias used to live here; callers that need
// sharing wrap `Coalesce` in `Arc` themselves.

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    #[tokio::test]
    async fn parallel_calls_share_one_fetch() {
        let c: Coalesce<u32> = Coalesce::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let c = Arc::new(c);

        let mut handles = Vec::new();
        for _ in 0..8 {
            let c = c.clone();
            let calls = calls.clone();
            handles.push(tokio::spawn(async move {
                c.run("k", || async {
                    calls.fetch_add(1, Ordering::SeqCst);
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                    Ok::<u32, std::convert::Infallible>(7)
                })
                .await
                .unwrap()
            }));
        }
        for h in handles {
            assert_eq!(h.await.unwrap(), 7);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }
}
