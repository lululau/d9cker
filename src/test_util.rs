//! Shared test helpers: one process-wide env lock + RAII restore.
//! All tests that read or mutate DOCKER_HOST / DOCKER_CONFIG / DOCKER_CONTEXT /
//! XDG_CONFIG_HOME must hold `ENV_LOCK` (and use `EnvGuard` when mutating) so
//! parallel test threads can't race on the process environment.

use tokio::sync::Mutex;

pub(crate) static ENV_LOCK: Mutex<()> = Mutex::const_new(());

pub(crate) struct EnvGuard(Vec<(&'static str, Option<String>)>);

impl EnvGuard {
    /// Apply (key, Some(value) | None=remove), remembering prior values to
    /// restore on drop.
    pub(crate) fn apply(changes: &[(&'static str, Option<&str>)]) -> Self {
        let saved = changes
            .iter()
            .map(|(k, _)| (*k, std::env::var(k).ok()))
            .collect();
        for (k, v) in changes {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
        EnvGuard(saved)
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for (k, v) in &self.0 {
            match v {
                Some(v) => std::env::set_var(k, v),
                None => std::env::remove_var(k),
            }
        }
    }
}
