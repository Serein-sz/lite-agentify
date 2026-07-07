use std::{
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::Duration,
};

use anyhow::{Context, bail};
use arc_swap::ArcSwap;
use notify::{RecursiveMode, Watcher};
use tracing::{error, info, warn};

use super::{
    config::{GatewayConfig, UsageDatabaseConfig},
    state::GatewayState,
};

const DEBOUNCE_WINDOW: Duration = Duration::from_millis(500);

/// The gateway state behind an atomically swappable pointer. Request handlers
/// load a snapshot at the start of a request and keep it for the request's
/// lifetime, so a concurrent reload never affects in-flight requests.
#[derive(Clone)]
pub struct SharedGatewayState {
    inner: Arc<Inner>,
}

struct Inner {
    state: ArcSwap<GatewayState>,
    reload: Option<ReloadContext>,
}

/// Boot-time values a reload needs: the file to re-read, and the
/// non-reloadable fields to compare against so changes can be warned about.
struct ReloadContext {
    config_path: PathBuf,
    listen_addr: std::net::SocketAddr,
    usage_database: Option<UsageDatabaseConfig>,
}

impl SharedGatewayState {
    pub(super) fn new(state: GatewayState, config: &GatewayConfig, config_path: PathBuf) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: ArcSwap::from_pointee(state),
                reload: Some(ReloadContext {
                    config_path,
                    listen_addr: config.listen_addr,
                    usage_database: config.usage_database.clone(),
                }),
            }),
        }
    }

    /// A shared state that cannot be reloaded; `/reload` reports an error.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(super) fn without_reload(state: GatewayState) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: ArcSwap::from_pointee(state),
                reload: None,
            }),
        }
    }

    pub(super) fn load(&self) -> Arc<GatewayState> {
        self.inner.state.load_full()
    }

    fn store(&self, state: GatewayState) {
        self.inner.state.store(Arc::new(state));
    }
}

/// Re-reads the config file and atomically swaps in a freshly validated state.
/// On any failure the currently active state is left untouched.
pub(super) fn reload(shared: &SharedGatewayState) -> anyhow::Result<()> {
    let Some(context) = shared.inner.reload.as_ref() else {
        bail!("hot reload is not configured for this gateway instance");
    };

    let config = GatewayConfig::load_from_path(&context.config_path)?;

    if config.listen_addr != context.listen_addr {
        warn!(
            active = %context.listen_addr,
            configured = %config.listen_addr,
            "listen_addr changed in config file; change requires a restart to take effect"
        );
    }
    if config.usage_database != context.usage_database {
        warn!("usage_database changed in config file; change requires a restart to take effect");
    }

    let current = shared.load();
    let next = GatewayState::from_config_with_upstream_and_recorder(
        config,
        current.upstream.clone(),
        current.usage_recorder.clone(),
    )?;
    shared.store(next);

    info!(
        config_path = %context.config_path.display(),
        "gateway configuration reloaded"
    );
    Ok(())
}

/// Watches the config file's directory and reloads on changes. Watching the
/// directory instead of the file survives editors that replace the file via
/// rename; bursts of events are debounced into one reload. Watcher failures
/// only disable auto-reload — proxying and `POST /reload` keep working.
pub fn spawn_config_watcher(shared: SharedGatewayState) {
    let Some(context) = shared.inner.reload.as_ref() else {
        warn!("config watcher not started: hot reload is not configured");
        return;
    };
    let config_path = context.config_path.clone();

    std::thread::Builder::new()
        .name("config-watcher".to_owned())
        .spawn(move || {
            if let Err(error) = watch_config(&shared, &config_path) {
                warn!(
                    error = format!("{error:#}"),
                    config_path = %config_path.display(),
                    "config watcher stopped; automatic reload disabled, POST /reload remains available"
                );
            }
        })
        .map(|_| ())
        .unwrap_or_else(|error| {
            warn!(%error, "failed to spawn config watcher thread");
        });
}

fn watch_config(shared: &SharedGatewayState, config_path: &Path) -> anyhow::Result<()> {
    let watch_dir = config_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .context("config path has no parent directory to watch")?;
    let file_name = config_path
        .file_name()
        .context("config path has no file name")?
        .to_owned();

    let (tx, rx) = mpsc::channel();
    let mut watcher =
        notify::recommended_watcher(tx).context("failed to create filesystem watcher")?;
    watcher
        .watch(watch_dir, RecursiveMode::NonRecursive)
        .with_context(|| format!("failed to watch {}", watch_dir.display()))?;

    info!(config_path = %config_path.display(), "watching gateway config file for changes");

    loop {
        let event = rx.recv().context("watcher event channel closed")?;
        if !event_touches_config(&event, &file_name) {
            continue;
        }

        // Debounce: wait for the burst of events from an editor save (write,
        // rename, metadata) to go quiet before reloading once.
        loop {
            match rx.recv_timeout(DEBOUNCE_WINDOW) {
                Ok(_) => continue,
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    bail!("watcher event channel closed")
                }
            }
        }

        if let Err(error) = reload(shared) {
            error!(
                error = format!("{error:#}"),
                config_path = %config_path.display(),
                "config reload triggered by file change failed; keeping previous configuration"
            );
        }
    }
}

fn event_touches_config(
    event: &Result<notify::Event, notify::Error>,
    file_name: &std::ffi::OsStr,
) -> bool {
    match event {
        // Events without paths cannot be attributed; treat them as relevant so
        // a reload attempt is never missed (a spurious reload is harmless).
        Ok(event) => {
            event.paths.is_empty()
                || event
                    .paths
                    .iter()
                    .any(|path| path.file_name() == Some(file_name))
        }
        Err(_) => false,
    }
}
