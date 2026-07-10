use std::{
    path::{Path, PathBuf},
    sync::{Arc, mpsc},
    time::Duration,
};

use anyhow::{Context, bail};
use arc_swap::ArcSwap;
use notify::{RecursiveMode, Watcher};
use tracing::{error, info, warn};

use crate::{
    config::{DatabaseConfig, GatewayConfig},
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
    /// Database-sourced provider + pricing config that seeds the snapshot.
    /// Carried across file reloads (which only re-read routes and retry) and
    /// replaced when the provider/pricing management APIs mutate the database.
    catalog: ArcSwap<crate::catalog::CatalogSnapshot>,
}

/// Boot-time values a reload needs: the file to re-read, and the
/// non-reloadable fields to compare against so changes can be warned about.
struct ReloadContext {
    config_path: PathBuf,
    listen_addr: std::net::SocketAddr,
    database: Option<DatabaseConfig>,
}

impl SharedGatewayState {
    pub(crate) fn new(
        state: GatewayState,
        config: &GatewayConfig,
        config_path: PathBuf,
        catalog: crate::catalog::CatalogSnapshot,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: ArcSwap::from_pointee(state),
                reload: Some(ReloadContext {
                    config_path,
                    listen_addr: config.listen_addr,
                    database: config.database.clone(),
                }),
                catalog: ArcSwap::from_pointee(catalog),
            }),
        }
    }

    /// A shared state that cannot be reloaded; `/reload` reports an error.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn without_reload(state: GatewayState) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: ArcSwap::from_pointee(state),
                reload: None,
                catalog: ArcSwap::from_pointee(crate::catalog::CatalogSnapshot::default()),
            }),
        }
    }

    pub(crate) fn load(&self) -> Arc<GatewayState> {
        self.inner.state.load_full()
    }

    fn store(&self, state: GatewayState) {
        self.inner.state.store(Arc::new(state));
    }

    /// Swaps in the same snapshot with a fresh key map. Called after account
    /// mutations so key/user changes take effect without re-reading the file.
    pub(crate) fn store_api_keys(&self, api_keys: crate::account::ApiKeyMap) {
        let next = self.load().with_api_keys(api_keys);
        self.store(next);
    }

    /// Swaps in the same snapshot with a fresh granted-credit map. Called
    /// after grant mutations so balances take effect immediately.
    pub(crate) fn store_granted(
        &self,
        granted: std::collections::HashMap<uuid::Uuid, rust_decimal::Decimal>,
    ) {
        let next = self.load().with_granted(granted);
        self.store(next);
    }

    /// The active database-sourced provider + pricing configuration.
    pub(crate) fn catalog(&self) -> Arc<crate::catalog::CatalogSnapshot> {
        self.inner.catalog.load_full()
    }

    /// Replaces the cached catalog and rebuilds the gateway snapshot from it
    /// plus the current file config (retry). Called after a provider, pricing,
    /// or model mutation. Returns an error without swapping if the new catalog
    /// fails validation (e.g. a deployment now references a missing provider),
    /// leaving the previous snapshot serving.
    pub(crate) fn store_catalog(
        &self,
        catalog: crate::catalog::CatalogSnapshot,
    ) -> anyhow::Result<()> {
        let context = self
            .inner
            .reload
            .as_ref()
            .context("catalog refresh requires hot reload to be configured")?;
        let config = GatewayConfig::load_from_path(&context.config_path)?;

        let current = self.load();
        let next = GatewayState::from_parts(
            config,
            catalog.clone(),
            current.upstream.clone(),
            current.usage_recorder.clone(),
        )?
        .with_api_keys((*current.api_keys).clone())
        .with_granted((*current.granted).clone())
        .with_spend_counter(current.spend_counter.clone());

        self.inner.catalog.store(Arc::new(catalog));
        self.store(next);
        Ok(())
    }
}

/// Re-reads the config file and atomically swaps in a freshly validated state.
/// On any failure the currently active state is left untouched.
pub(crate) fn reload(shared: &SharedGatewayState) -> anyhow::Result<()> {
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
    if config.database != context.database {
        warn!("database changed in config file; change requires a restart to take effect");
    }

    // Providers, pricing, and the model catalog come from the database, not
    // the file: overlay the cached catalog so a file reload only re-reads the
    // retry policy (and warns about restart-only fields).
    let catalog = shared.catalog();

    let current = shared.load();
    let next = GatewayState::from_parts(
        config,
        (*catalog).clone(),
        current.upstream.clone(),
        current.usage_recorder.clone(),
    )?
    // A file reload never changes accounts, balances, or counters: carry the
    // database-owned state over.
    .with_api_keys((*current.api_keys).clone())
    .with_granted((*current.granted).clone())
    .with_spend_counter(current.spend_counter.clone());
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
    use notify::event::{AccessKind, AccessMode, EventKind};

    match event {
        // Events without paths cannot be attributed; treat them as relevant so
        // a reload attempt is never missed (a spurious reload is harmless).
        Ok(event) => {
            match event.kind {
                // Close-after-write is the canonical "editor saved the file
                // in place" signal; keep it.
                EventKind::Access(AccessKind::Close(AccessMode::Write)) => {}
                // Other access events (open/read/close-nowrite) fire on mere
                // reads. The reload itself reads the config file, so reacting
                // to reads would re-trigger the watcher forever.
                EventKind::Access(_) => return false,
                _ => {}
            }
            event.paths.is_empty()
                || event
                    .paths
                    .iter()
                    .any(|path| path.file_name() == Some(file_name))
        }
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{
        AccessKind, AccessMode, CreateKind, DataChange, EventKind, ModifyKind,
    };

    fn event(kind: EventKind, path: Option<&str>) -> Result<notify::Event, notify::Error> {
        let mut event = notify::Event::new(kind);
        if let Some(path) = path {
            event = event.add_path(std::path::PathBuf::from(path));
        }
        Ok(event)
    }

    /// Reads of the config file (open/read/close-nowrite) must not trigger a
    /// reload: the reload itself reads the file, so reacting to read events
    /// would re-trigger the watcher in a 500ms loop forever.
    #[test]
    fn read_access_events_are_ignored() {
        let name = std::ffi::OsStr::new("gateway.toml");
        for kind in [
            EventKind::Access(AccessKind::Open(AccessMode::Any)),
            EventKind::Access(AccessKind::Read),
            EventKind::Access(AccessKind::Close(AccessMode::Read)),
        ] {
            assert!(
                !event_touches_config(&event(kind, Some("/etc/gateway.toml")), name),
                "{kind:?} is a read and must not trigger a reload"
            );
        }
    }

    #[test]
    fn writes_saves_and_unattributable_events_still_trigger() {
        let name = std::ffi::OsStr::new("gateway.toml");
        for kind in [
            EventKind::Access(AccessKind::Close(AccessMode::Write)),
            EventKind::Modify(ModifyKind::Data(DataChange::Any)),
            EventKind::Create(CreateKind::File),
        ] {
            assert!(
                event_touches_config(&event(kind, Some("/etc/gateway.toml")), name),
                "{kind:?} changes the file and must trigger a reload"
            );
        }
        // Events without paths cannot be attributed and stay relevant.
        assert!(event_touches_config(&event(EventKind::Other, None), name));
        // Changes to sibling files stay irrelevant.
        assert!(!event_touches_config(
            &event(
                EventKind::Modify(ModifyKind::Data(DataChange::Any)),
                Some("/etc/other.toml")
            ),
            name
        ));
    }
}
