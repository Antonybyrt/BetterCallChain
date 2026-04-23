use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::event_bus::EventBus;
use crate::parser::parse_line;

pub struct LogReader {
    container: String,
    node_name: String,
    bus: Arc<EventBus>,
}

impl LogReader {
    pub fn new(container: &str, node_name: &str, bus: Arc<EventBus>) -> Self {
        Self {
            container: container.to_string(),
            node_name: node_name.to_string(),
            bus,
        }
    }

    pub fn spawn(self, cancel: CancellationToken) {
        let reader = Arc::new(self);
        tokio::spawn(async move {
            let mut backoff = Duration::from_secs(2);
            loop {
                tokio::select! {
                    biased;
                    _ = cancel.cancelled() => return,
                    _ = reader.run_once() => {}
                }
                warn!(container = %reader.container, "docker log stream ended, retrying in {:?}", backoff);
                tokio::select! {
                    _ = cancel.cancelled() => return,
                    _ = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(Duration::from_secs(60));
            }
        });
    }

    async fn run_once(&self) {
        // docker logs writes container-stdout to its stdout and container-stderr to its stderr.
        // tracing_subscriber defaults to stderr, but Docker may also mirror to stdout.
        // We capture BOTH and parse from each in parallel tasks.
        let mut cmd = tokio::process::Command::new("docker");
        cmd.args(["logs", "--follow", &self.container])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                warn!(container = %self.container, err = %e, "failed to spawn docker logs");
                return;
            }
        };

        info!(container = %self.container, "started following logs");

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();
        let bus = Arc::clone(&self.bus);
        let node = self.node_name.clone();

        // Track lines already seen to deduplicate if docker logs emits the same line on both streams
        let seen: Arc<tokio::sync::Mutex<lru::LruCache<u64, ()>>> =
            Arc::new(tokio::sync::Mutex::new(
                lru::LruCache::new(std::num::NonZeroUsize::new(256).unwrap()),
            ));

        let mut tasks = Vec::new();

        macro_rules! spawn_reader {
            ($stream:expr) => {
                if let Some(s) = $stream {
                    let bus = Arc::clone(&bus);
                    let node = node.clone();
                    let seen = Arc::clone(&seen);
                    tasks.push(tokio::spawn(async move {
                        let reader = BufReader::new(s);
                        let mut lines = reader.lines();
                        while let Ok(Some(raw)) = lines.next_line().await {
                            use std::hash::{Hash, Hasher};
                            let mut h = std::collections::hash_map::DefaultHasher::new();
                            raw.hash(&mut h);
                            let key = h.finish();
                            {
                                let mut cache = seen.lock().await;
                                if cache.put(key, ()).is_some() {
                                    continue;
                                }
                            }
                            if let Some(parsed) = parse_line(&raw) {
                                bus.publish_raw(node.clone(), parsed.timestamp, parsed.level, parsed.event);
                            }
                        }
                    }));
                }
            };
        }

        spawn_reader!(stdout);
        spawn_reader!(stderr);

        for t in tasks {
            let _ = t.await;
        }
        let _ = child.wait().await;
    }
}
