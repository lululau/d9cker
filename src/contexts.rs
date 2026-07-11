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
