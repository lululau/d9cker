//! Docker context discovery.
//!
//! bollard talks to a single endpoint; it does NOT understand Docker's
//! `~/.docker/contexts` store. So we read the store ourselves to enumerate
//! contexts and resolve each one to an endpoint host string
//! (`ssh://…`, `unix://…`, `tcp://…`), which we then hand to bollard.

use anyhow::Result;
use serde::Deserialize;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct DockerContext {
    pub name: String,
    pub description: String,
    pub host: String,
}

#[derive(Deserialize)]
struct Meta {
    #[serde(rename = "Name")]
    name: String,
    #[serde(rename = "Metadata", default)]
    metadata: MetaInner,
    #[serde(rename = "Endpoints", default)]
    endpoints: Endpoints,
}

#[derive(Deserialize, Default)]
struct MetaInner {
    #[serde(rename = "Description", default)]
    description: String,
}

#[derive(Deserialize, Default)]
struct Endpoints {
    #[serde(rename = "docker", default)]
    docker: Endpoint,
}

#[derive(Deserialize, Default)]
struct Endpoint {
    #[serde(rename = "Host", default)]
    host: String,
}

#[derive(Deserialize)]
struct CliConfig {
    #[serde(rename = "currentContext", default)]
    current_context: String,
}

fn docker_dir() -> PathBuf {
    if let Ok(d) = std::env::var("DOCKER_CONFIG") {
        return PathBuf::from(d);
    }
    let home = std::env::var("HOME").unwrap_or_default();
    PathBuf::from(home).join(".docker")
}

/// The built-in "default" context (not stored on disk).
fn default_context() -> DockerContext {
    let host = std::env::var("DOCKER_HOST")
        .unwrap_or_else(|_| "unix:///var/run/docker.sock".to_string());
    DockerContext {
        name: "default".into(),
        description: "Current DOCKER_HOST based configuration".into(),
        host,
    }
}

/// Enumerate all contexts: the built-in default plus everything in the store.
pub fn load_contexts() -> Vec<DockerContext> {
    let mut out = vec![default_context()];
    let meta_dir = docker_dir().join("contexts").join("meta");
    if let Ok(entries) = std::fs::read_dir(&meta_dir) {
        for entry in entries.flatten() {
            let path = entry.path().join("meta.json");
            if let Ok(text) = std::fs::read_to_string(&path) {
                if let Ok(meta) = serde_json::from_str::<Meta>(&text) {
                    if meta.name.is_empty() {
                        continue;
                    }
                    out.push(DockerContext {
                        name: meta.name,
                        description: meta.metadata.description,
                        host: meta.endpoints.docker.host,
                    });
                }
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// The context Docker would use right now: $DOCKER_CONTEXT, else the CLI
/// config's `currentContext`, else "default".
pub fn current_context() -> String {
    if let Ok(c) = std::env::var("DOCKER_CONTEXT") {
        if !c.is_empty() {
            return c;
        }
    }
    let cfg = docker_dir().join("config.json");
    if let Ok(text) = std::fs::read_to_string(cfg) {
        if let Ok(c) = serde_json::from_str::<CliConfig>(&text) {
            if !c.current_context.is_empty() {
                return c.current_context;
            }
        }
    }
    "default".into()
}

/// Resolve a context name to its endpoint host string.
pub fn resolve_host(name: &str) -> Result<String> {
    load_contexts()
        .into_iter()
        .find(|c| c.name == name)
        .map(|c| c.host)
        .ok_or_else(|| anyhow::anyhow!("unknown context '{name}'"))
}

/// Split an `ssh://user@host[:port]` endpoint into (user@host, port).
/// Returns None for non-ssh endpoints (local socket / tcp).
pub fn ssh_target(host: &str) -> Option<(String, Option<String>)> {
    let rest = host.strip_prefix("ssh://")?;
    if let Some((hostpart, port)) = rest.rsplit_once(':') {
        if !port.is_empty() && port.chars().all(|c| c.is_ascii_digit()) {
            return Some((hostpart.to_string(), Some(port.to_string())));
        }
    }
    Some((rest.to_string(), None))
}

/// Single-quote a path for a remote shell.
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Read a file that lives on the context's engine host: over ssh for remote
/// contexts, straight off the filesystem for local ones. (There is no Engine
/// API for the host filesystem, so ssh is the honest route here.)
pub async fn read_file(host: &str, path: &str) -> Result<String> {
    if let Some((target, port)) = ssh_target(host) {
        let mut c = tokio::process::Command::new("ssh");
        c.arg("-o").arg("BatchMode=yes");
        if let Some(p) = port {
            c.arg("-p").arg(p);
        }
        c.arg(target).arg("cat").arg("--").arg(path);
        let out = c.output().await?;
        if !out.status.success() {
            return Err(anyhow::anyhow!(
                String::from_utf8_lossy(&out.stderr).trim().to_string()
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Ok(std::fs::read_to_string(path)?)
    }
}

#[cfg(test)]
mod tests {
    use super::{sh_quote, ssh_target};

    #[test]
    fn ssh_target_parses() {
        assert_eq!(
            ssh_target("ssh://yizhi@10.0.0.106"),
            Some(("yizhi@10.0.0.106".into(), None))
        );
        assert_eq!(
            ssh_target("ssh://user@host:2222"),
            Some(("user@host".into(), Some("2222".into())))
        );
        assert_eq!(ssh_target("unix:///var/run/docker.sock"), None);
    }

    #[test]
    fn sh_quote_escapes() {
        assert_eq!(sh_quote("/a/b.yml"), "'/a/b.yml'");
        assert_eq!(sh_quote("/it's/x.yml"), "'/it'\\''s/x.yml'");
    }
}
