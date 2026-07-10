//! The reserved `config_changed` Redis channel (design D7).
//!
//! Every snapshot-affecting console mutation publishes here after the local
//! snapshot has already been refreshed inline. In this single-instance
//! release the subscriber is deliberately a no-op self-notification — it
//! exists so a future multi-instance deployment can turn the message into a
//! "rebuild your snapshot from the database" trigger without a protocol
//! change. Publishing is best-effort: a failure only means other (not yet
//! existing) instances would miss the nudge.

use futures_util::StreamExt;
use tracing::debug;

pub(crate) const CONFIG_CHANGED_CHANNEL: &str = "config_changed";

/// Best-effort publisher for snapshot-affecting mutations. Cheap to clone;
/// present only when `[redis]` is configured.
#[derive(Clone)]
pub(crate) struct ConfigNotifier {
    connection: redis::aio::ConnectionManager,
}

impl ConfigNotifier {
    pub(crate) fn new(connection: redis::aio::ConnectionManager) -> Self {
        Self { connection }
    }

    /// Publishes `what` (e.g. "catalog", "api_keys", "granted") without
    /// blocking the mutation response on the round trip.
    pub(crate) fn publish(&self, what: &'static str) {
        let mut connection = self.connection.clone();
        tokio::spawn(async move {
            let result: Result<i64, _> = redis::cmd("PUBLISH")
                .arg(CONFIG_CHANGED_CHANNEL)
                .arg(what)
                .query_async(&mut connection)
                .await;
            if let Err(error) = result {
                debug!(%error, what, "config_changed publish failed (advisory only)");
            }
        });
    }
}

/// Subscribes to `config_changed` for the lifetime of the process. The
/// handler is a no-op: this instance already refreshed its snapshot before
/// publishing, so its own message carries no new information. Reconnects
/// quietly if the subscription drops.
pub(crate) fn spawn_config_subscriber(client: redis::Client) {
    tokio::spawn(async move {
        loop {
            match client.get_async_pubsub().await {
                Ok(mut pubsub) => {
                    if let Err(error) = pubsub.subscribe(CONFIG_CHANGED_CHANNEL).await {
                        debug!(%error, "config_changed subscribe failed; retrying");
                    } else {
                        let mut stream = pubsub.on_message();
                        while let Some(message) = stream.next().await {
                            let what: String = message.get_payload().unwrap_or_default();
                            debug!(
                                what,
                                "config_changed received; snapshot already refreshed locally (single instance)"
                            );
                        }
                    }
                }
                Err(error) => {
                    debug!(%error, "config_changed subscriber cannot connect; retrying");
                }
            }
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        }
    });
}
