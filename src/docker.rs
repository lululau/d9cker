//! Docker data layer, backed by the native bollard API.
//!
//! bollard talks to a single endpoint; context enumeration/resolution lives in
//! `contexts`. Given a resolved host string we `connect()` here and drive all
//! reads/actions/log-streams over the native API. The one exception is
//! interactive `exec`, handled in `main` via the `docker` CLI.

use crate::contexts;
use anyhow::{anyhow, Result};
use bollard::query_parameters::{
    DataUsageOptions, InspectContainerOptions, InspectNetworkOptions, InspectServiceOptions,
    ListContainersOptionsBuilder, ListImagesOptionsBuilder, ListNetworksOptions, ListNodesOptions,
    ListServicesOptions, ListTasksOptionsBuilder, ListVolumesOptions, LogsOptionsBuilder,
    PruneBuildOptions, PruneContainersOptions, PruneImagesOptions, PruneImagesOptionsBuilder,
    PruneNetworksOptions, RemoveContainerOptionsBuilder, RemoveImageOptionsBuilder,
    RemoveVolumeOptionsBuilder, RestartContainerOptions, StartContainerOptions,
    StatsOptionsBuilder, StopContainerOptions, UpdateServiceOptionsBuilder,
};
use bollard::{Docker, API_DEFAULT_VERSION};
use futures::stream::{BoxStream, StreamExt};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

const TIMEOUT: u64 = 120;

/// The resource views d9cker can browse, k9s-style.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum View {
    Containers,
    Images,
    Services,
    Nodes,
    Contexts,
    ServiceTasks,
    Volumes,
    Networks,
    Compose,
    Stacks,
    StackTasks,
}

impl View {
    pub fn title(self) -> &'static str {
        match self {
            View::Containers => "Containers",
            View::Images => "Images",
            View::Services => "Services",
            View::Nodes => "Nodes",
            View::Contexts => "Contexts",
            View::ServiceTasks => "Service Tasks",
            View::Volumes => "Volumes",
            View::Networks => "Networks",
            View::Compose => "Compose",
            View::Stacks => "Stacks",
            View::StackTasks => "Stack Tasks",
        }
    }

    pub fn columns(self) -> &'static [&'static str] {
        match self {
            View::Containers => &["ID", "NAME", "IMAGE", "STATE", "STATUS", "IP", "PORTS"],
            View::Images => &["REPOSITORY", "TAG", "ID", "SIZE", "CREATED"],
            View::Services => &["NAME", "MODE", "REPLICAS", "IMAGE", "PORTS"],
            View::Nodes => &["HOSTNAME", "IP", "STATUS", "AVAILABILITY", "MANAGER", "ENGINE"],
            View::Contexts => &["", "NAME", "DESCRIPTION", "ENDPOINT"],
            View::ServiceTasks => &["NAME", "NODE", "DESIRED", "CURRENT", "IMAGE", "ERROR"],
            View::Volumes => &["NAME", "DRIVER", "SIZE", "SCOPE", "MOUNTPOINT"],
            View::Networks => &["NAME", "DRIVER", "SCOPE", "SUBNET", "ID"],
            View::Compose => &["PROJECT", "STATUS", "CONTAINERS", "CONFIG FILES"],
            View::Stacks => &["NAME", "SERVICES", "ORCHESTRATOR"],
            View::StackTasks => &["NAME", "NODE", "IP", "DESIRED", "CURRENT", "ERROR"],
        }
    }

    /// Sensible maximum width per column. Columns shrink to fit their content
    /// but never grow past this, so one pathologically long value can't stretch
    /// the table out of shape.
    pub fn col_max(self) -> &'static [u16] {
        match self {
            View::Containers => &[12, 32, 34, 9, 22, 16, 26],
            View::Images => &[40, 18, 14, 9, 8],
            View::Services => &[32, 11, 9, 34, 18],
            View::Nodes => &[20, 16, 9, 13, 10, 9],
            View::Contexts => &[2, 12, 26, 44],
            View::ServiceTasks => &[32, 14, 10, 12, 30, 28],
            View::Volumes => &[30, 8, 9, 7, 46],
            View::Networks => &[26, 10, 7, 18, 14],
            View::Compose => &[24, 12, 11, 52],
            View::Stacks => &[32, 9, 13],
            View::StackTasks => &[40, 14, 16, 10, 12, 28],
        }
    }

    pub fn inspect_type(self) -> Option<&'static str> {
        match self {
            View::Containers => Some("container"),
            View::Images => Some("image"),
            View::Services => Some("service"),
            View::Nodes => Some("node"),
            // a stack task's row id is its container id, so inspect the container
            View::StackTasks => Some("container"),
            View::Volumes => Some("volume"),
            View::Networks => Some("network"),
            _ => None,
        }
    }
}

/// One row in a resource table.
#[derive(Clone, Debug, Default)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub cells: Vec<String>,
    /// A non-selectable group header row (e.g. a service heading in the grouped
    /// stack-tasks view), not a real resource.
    pub header: bool,
}

// ---- helpers -----------------------------------------------------------

/// Render a serde-serializable value (typically a bollard enum) as a clean
/// display string, without depending on each type's Display impl.
fn estr<T: Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(Value::String(s)) => s,
        Ok(other) => other.to_string().trim_matches('"').to_string(),
        Err(_) => String::new(),
    }
}

fn opt_estr<T: Serialize>(v: &Option<T>) -> String {
    v.as_ref().map(estr).unwrap_or_default()
}

fn short(id: &str) -> String {
    let id = id.strip_prefix("sha256:").unwrap_or(id);
    id.chars().take(12).collect()
}

/// Strip a trailing `@sha256:…` digest from an image ref for display.
fn clean_image(img: &str) -> String {
    img.split_once("@sha256:")
        .map(|(h, _)| h)
        .unwrap_or(img)
        .to_string()
}

pub fn human_size(bytes: i64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{bytes}B")
    } else {
        format!("{v:.1}{}", U[i])
    }
}

fn fmt_age(unix_secs: i64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let d = (now - unix_secs).max(0);
    match d {
        0..=59 => format!("{d}s"),
        60..=3599 => format!("{}m", d / 60),
        3600..=86399 => format!("{}h", d / 3600),
        _ => format!("{}d", d / 86400),
    }
}

// ---- connection --------------------------------------------------------

/// Connect bollard to a resolved endpoint host string.
pub fn connect(host: &str) -> Result<Docker> {
    let d = if host.starts_with("ssh://") {
        Docker::connect_with_ssh(host, TIMEOUT, API_DEFAULT_VERSION, None)?
    } else if host.starts_with("unix://") || host.starts_with("npipe://") {
        Docker::connect_with_unix(host, TIMEOUT, API_DEFAULT_VERSION)?
    } else if host.is_empty() {
        Docker::connect_with_defaults()?
    } else {
        Docker::connect_with_http(host, TIMEOUT, API_DEFAULT_VERSION)?
    };
    Ok(d)
}

// ---- listing -----------------------------------------------------------

/// List items for a view. `arg` = current-context name (Contexts) or service
/// name (ServiceTasks).
pub async fn list(docker: &Docker, view: View, arg: &str) -> Result<Vec<Item>> {
    let items = match view {
        View::Containers => {
            // arg == "all" -> include exited/stopped; otherwise running only
            let opts = ListContainersOptionsBuilder::default()
                .all(arg == "all")
                .build();
            docker
                .list_containers(Some(opts))
                .await?
                .into_iter()
                .map(|c| {
                    let id = c.id.unwrap_or_default();
                    let name = c
                        .names
                        .and_then(|n| n.into_iter().next())
                        .unwrap_or_default()
                        .trim_start_matches('/')
                        .to_string();
                    let ports = fmt_ports(&c.ports);
                    let ip = c
                        .network_settings
                        .as_ref()
                        .and_then(|ns| ns.networks.as_ref())
                        .map(|nets| {
                            let mut ips: Vec<String> = nets
                                .values()
                                .filter_map(|e| e.ip_address.clone())
                                .filter(|s| !s.is_empty())
                                .collect();
                            ips.sort();
                            ips.dedup();
                            ips.join(", ")
                        })
                        .unwrap_or_default();
                    let cells = vec![
                        short(&id),
                        name.clone(),
                        clean_image(&c.image.unwrap_or_default()),
                        opt_estr(&c.state),
                        c.status.unwrap_or_default(),
                        ip,
                        ports,
                    ];
                    Item { id, name, cells, ..Default::default() }
                })
                .collect()
        }
        View::Images => {
            // grouped by repository namespace (the path before the last '/'):
            // a header per namespace, its images indented underneath
            let opts = ListImagesOptionsBuilder::default().build();
            let imgs = docker.list_images(Some(opts)).await?;

            struct Row {
                name: String,
                tag: String,
                id: String,
                size: String,
                created: String,
            }
            let mut groups: std::collections::BTreeMap<String, Vec<Row>> =
                std::collections::BTreeMap::new();
            for im in imgs {
                let (repo, tag) = im
                    .repo_tags
                    .first()
                    .and_then(|rt| rt.rsplit_once(':'))
                    .map(|(r, t)| (r.to_string(), t.to_string()))
                    .unwrap_or_else(|| ("<none>".into(), "<none>".into()));
                let (group, short_name) = if repo == "<none>" {
                    ("(untagged)".to_string(), "<none>".to_string())
                } else if let Some((ns, name)) = repo.rsplit_once('/') {
                    (ns.to_string(), name.to_string())
                } else {
                    ("(top-level)".to_string(), repo.clone())
                };
                groups.entry(group).or_default().push(Row {
                    name: short_name,
                    tag,
                    id: im.id,
                    size: human_size(im.size),
                    created: fmt_age(im.created),
                });
            }

            let mut items = Vec::new();
            for (group, mut rows) in groups {
                rows.sort_by(|a, b| a.name.cmp(&b.name).then(a.tag.cmp(&b.tag)));
                items.push(Item {
                    cells: vec![
                        format!("{group}  ({})", rows.len()),
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                    ],
                    name: group,
                    header: true,
                    ..Default::default()
                });
                for r in rows {
                    items.push(Item {
                        name: format!("{}:{}", r.name, r.tag),
                        cells: vec![
                            format!("  {}", r.name),
                            r.tag,
                            short(&r.id),
                            r.size,
                            r.created,
                        ],
                        id: r.id,
                        ..Default::default()
                    });
                }
            }
            items
        }
        View::Services => {
            let svcs = docker.list_services(None::<ListServicesOptions>).await?;
            svcs.iter().map(service_item).collect()
        }
        View::StackTasks => {
            // `docker stack ps <name>`, grouped by service: a header row per
            // service followed by its task instances (indented). Task IP comes
            // from the manager's own NetworksAttachments (cluster-wide), with the
            // ingress subnet filtered out.
            let mut filters: HashMap<String, Vec<String>> = HashMap::new();
            filters.insert(
                "label".to_string(),
                vec![format!("com.docker.stack.namespace={arg}")],
            );
            let opts = ListTasksOptionsBuilder::default().filters(&filters).build();
            let tasks = docker.list_tasks(Some(opts)).await?;
            let nodes = node_hostnames(docker).await;
            let svc_meta = service_meta(docker).await;
            let cips = container_ips(docker).await;
            let ingress = ingress_subnets(docker).await;

            struct Row {
                name: String,
                node: String,
                ip: String,
                desired: String,
                current: String,
                err: String,
                id: String,
                slot: i64,
            }
            // group tasks by service (BTreeMap keeps services in name order)
            let mut groups: std::collections::BTreeMap<String, (String, String, Vec<Row>)> =
                std::collections::BTreeMap::new();
            for t in tasks {
                let tid = t.id.clone().unwrap_or_default();
                let node_id = t.node_id.clone().unwrap_or_default();
                let slot_num = t.slot.unwrap_or(0);
                let slot = t.slot.map(|n| n.to_string()).unwrap_or_default();
                let sid = t.service_id.clone().unwrap_or_default();
                let (svc, mode, declared) = svc_meta.get(&sid).cloned().unwrap_or_default();
                let svc_key = if svc.is_empty() { short(&sid) } else { svc.clone() };
                // bollard leaves Task.name empty; rebuild service.slot (or
                // service.node for global-mode tasks that have no slot)
                let name = t.name.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| {
                    if svc.is_empty() {
                        short(&tid)
                    } else if !slot.is_empty() {
                        format!("{svc}.{slot}")
                    } else {
                        format!("{svc}.{}", short(&node_id))
                    }
                });
                let node = nodes
                    .get(&node_id)
                    .cloned()
                    .unwrap_or_else(|| short(&node_id));
                let cont = t
                    .status
                    .as_ref()
                    .and_then(|s| s.container_status.as_ref())
                    .and_then(|cs| cs.container_id.clone())
                    .unwrap_or_default();
                // authoritative source = the task's own NetworksAttachments;
                // fall back to the local container map (host-net / edge cases)
                let mut ip = task_ip(&t, &ingress);
                if ip.is_empty() {
                    ip = cips.get(&cont).cloned().unwrap_or_default();
                }
                let desired = opt_estr(&t.desired_state);
                let (current, err) = t
                    .status
                    .as_ref()
                    .map(|s| (opt_estr(&s.state), s.err.clone().unwrap_or_default()))
                    .unwrap_or_default();
                // row id = container id so `i`/`l` target the real container
                let id = if cont.is_empty() { tid } else { cont };
                let entry = groups
                    .entry(svc_key)
                    .or_insert_with(|| (mode, declared, Vec::new()));
                entry.2.push(Row {
                    name,
                    node,
                    ip,
                    desired,
                    current,
                    err,
                    id,
                    slot: slot_num,
                });
            }

            // service/task names carry the stack prefix (`stack_svc`); strip it —
            // the stack is already the context we're inside.
            let pfx = format!("{arg}_");
            let strip = |s: &str| s.strip_prefix(&pfx).unwrap_or(s).to_string();
            let mut items = Vec::new();
            for (svc, (mode, declared, mut rows)) in groups {
                // running tasks first, then by slot
                rows.sort_by(|a, b| {
                    let (ar, br) = (a.current == "running", b.current == "running");
                    br.cmp(&ar).then(a.slot.cmp(&b.slot))
                });
                let running = rows.iter().filter(|r| r.current == "running").count();
                let summary = if !declared.is_empty() {
                    format!("{mode} {running}/{declared}")
                } else if !mode.is_empty() {
                    format!("{mode} {running}")
                } else {
                    format!("{running} running")
                };
                items.push(Item {
                    name: svc.clone(),
                    cells: vec![
                        format!("{}  ({summary})", strip(&svc)),
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                        String::new(),
                    ],
                    header: true,
                    ..Default::default()
                });
                for r in rows {
                    items.push(Item {
                        id: r.id,
                        cells: vec![
                            format!("  {}", strip(&r.name)),
                            r.node,
                            r.ip,
                            r.desired,
                            r.current,
                            r.err,
                        ],
                        name: r.name,
                        ..Default::default()
                    });
                }
            }
            items
        }
        View::Stacks => {
            // The Engine has no stack object — `docker stack ls` groups services
            // client-side by the com.docker.stack.namespace label. We do the same.
            let svcs = docker.list_services(None::<ListServicesOptions>).await?;
            let mut map: std::collections::BTreeMap<String, usize> =
                std::collections::BTreeMap::new();
            for s in &svcs {
                if let Some(ns) = stack_namespace(s) {
                    *map.entry(ns).or_insert(0) += 1;
                }
            }
            map.into_iter()
                .map(|(name, count)| {
                    let cells = vec![name.clone(), count.to_string(), "Swarm".to_string()];
                    Item {
                        id: name.clone(),
                        name,
                        cells,
                        ..Default::default()
                    }
                })
                .collect()
        }
        View::Nodes => {
            let nodes = docker.list_nodes(None::<ListNodesOptions>).await?;
            nodes
                .into_iter()
                .map(|n| {
                    let id = n.id.clone().unwrap_or_default();
                    let desc = n.description.as_ref();
                    let host = desc.and_then(|d| d.hostname.clone()).unwrap_or_default();
                    let addr = n
                        .status
                        .as_ref()
                        .and_then(|s| s.addr.clone())
                        .unwrap_or_default();
                    let status = n
                        .status
                        .as_ref()
                        .map(|s| opt_estr(&s.state))
                        .unwrap_or_default();
                    let avail = n
                        .spec
                        .as_ref()
                        .map(|s| opt_estr(&s.availability))
                        .unwrap_or_default();
                    let manager = match n.manager_status.as_ref() {
                        Some(ms) if ms.leader == Some(true) => "Leader".to_string(),
                        Some(_) => "Reachable".to_string(),
                        None => String::new(),
                    };
                    let engine = desc
                        .and_then(|d| d.engine.as_ref())
                        .and_then(|e| e.engine_version.clone())
                        .unwrap_or_default();
                    let cells = vec![host.clone(), addr, status, avail, manager, engine];
                    Item {
                        id,
                        name: host,
                        cells,
                        ..Default::default()
                    }
                })
                .collect()
        }
        View::ServiceTasks => {
            let mut filters: HashMap<String, Vec<String>> = HashMap::new();
            filters.insert("service".to_string(), vec![arg.to_string()]);
            let opts = ListTasksOptionsBuilder::default().filters(&filters).build();
            let tasks = docker.list_tasks(Some(opts)).await?;
            let nodes = node_hostnames(docker).await;
            tasks
                .into_iter()
                .map(|t| {
                    let id = t.id.clone().unwrap_or_default();
                    let slot = t.slot.map(|n| n.to_string()).unwrap_or_default();
                    // bollard usually leaves Task.name empty; reconstruct service.slot
                    let name = t.name.clone().filter(|s| !s.is_empty()).unwrap_or_else(|| {
                        if slot.is_empty() {
                            short(&id)
                        } else {
                            format!("{arg}.{slot}")
                        }
                    });
                    let node_id = t.node_id.clone().unwrap_or_default();
                    let node = nodes
                        .get(&node_id)
                        .cloned()
                        .unwrap_or_else(|| short(&node_id));
                    let desired = opt_estr(&t.desired_state);
                    let (current, err) = t
                        .status
                        .as_ref()
                        .map(|s| (opt_estr(&s.state), s.err.clone().unwrap_or_default()))
                        .unwrap_or_default();
                    let image = t
                        .spec
                        .as_ref()
                        .and_then(|sp| sp.container_spec.as_ref())
                        .and_then(|cs| cs.image.clone())
                        .map(|i| clean_image(&i))
                        .unwrap_or_default();
                    let cells = vec![name.clone(), node, desired, current, image, err];
                    Item { id, name, cells, ..Default::default() }
                })
                .collect()
        }
        View::Volumes => {
            let resp = docker.list_volumes(None::<ListVolumesOptions>).await?;
            resp.volumes
                .unwrap_or_default()
                .into_iter()
                .map(|v| {
                    // SIZE is filled in lazily by the app (passive df peek + cache)
                    let cells = vec![
                        v.name.clone(),
                        v.driver.clone(),
                        "?".to_string(),
                        opt_estr(&v.scope),
                        v.mountpoint.clone(),
                    ];
                    Item {
                        id: v.name.clone(),
                        name: v.name,
                        cells,
                        ..Default::default()
                    }
                })
                .collect()
        }
        View::Networks => {
            let nets = docker.list_networks(None::<ListNetworksOptions>).await?;
            nets.into_iter()
                .map(|n| {
                    let id = n.id.unwrap_or_default();
                    let name = n.name.unwrap_or_default();
                    let subnet = n
                        .ipam
                        .as_ref()
                        .and_then(|i| i.config.as_ref())
                        .map(|cfgs| {
                            cfgs.iter()
                                .filter_map(|c| c.subnet.clone())
                                .collect::<Vec<_>>()
                                .join(", ")
                        })
                        .unwrap_or_default();
                    let cells = vec![
                        name.clone(),
                        n.driver.unwrap_or_default(),
                        n.scope.unwrap_or_default(),
                        subnet,
                        short(&id),
                    ];
                    Item { id, name, cells, ..Default::default() }
                })
                .collect()
        }
        View::Compose => {
            let opts = ListContainersOptionsBuilder::default().all(true).build();
            let cs = docker.list_containers(Some(opts)).await?;
            // group by the compose project label — this is exactly what
            // `docker compose ls` does client-side (there is no Engine API for it)
            let mut map: std::collections::BTreeMap<String, (usize, usize, String)> =
                std::collections::BTreeMap::new();
            for c in cs {
                let labels = c.labels.clone().unwrap_or_default();
                let proj = match labels.get("com.docker.compose.project") {
                    Some(p) if !p.is_empty() => p.clone(),
                    _ => continue,
                };
                let cfg = labels
                    .get("com.docker.compose.project.config_files")
                    .cloned()
                    .unwrap_or_default();
                let running = opt_estr(&c.state) == "running";
                let e = map.entry(proj).or_insert((0, 0, String::new()));
                e.1 += 1;
                if running {
                    e.0 += 1;
                }
                if e.2.is_empty() && !cfg.is_empty() {
                    e.2 = cfg;
                }
            }
            map.into_iter()
                .map(|(name, (run, total, cfg))| {
                    let status = if run > 0 {
                        format!("running({run})")
                    } else {
                        format!("exited({total})")
                    };
                    let cells = vec![name.clone(), status, format!("{run}/{total}"), cfg];
                    Item {
                        id: name.clone(),
                        name,
                        cells,
                        ..Default::default()
                    }
                })
                .collect()
        }
        View::Contexts => contexts::load_contexts()
            .into_iter()
            .map(|c| {
                let current = c.name == arg;
                let cells = vec![
                    if current {
                        "●".to_string()
                    } else {
                        " ".to_string()
                    },
                    c.name.clone(),
                    c.description,
                    c.host,
                ];
                Item {
                    id: c.name.clone(),
                    name: c.name,
                    cells,
                    ..Default::default()
                }
            })
            .collect(),
    };
    Ok(items)
}

/// The swarm stack namespace a service belongs to, if any
/// (`com.docker.stack.namespace` label).
fn stack_namespace(s: &bollard::models::Service) -> Option<String> {
    s.spec
        .as_ref()
        .and_then(|sp| sp.labels.as_ref())
        .and_then(|l| l.get("com.docker.stack.namespace"))
        .filter(|v| !v.is_empty())
        .cloned()
}

/// Build a service table row: NAME, MODE, REPLICAS, IMAGE, PORTS.
/// Shared by the Services and StackServices views.
fn service_item(s: &bollard::models::Service) -> Item {
    let id = s.id.clone().unwrap_or_default();
    let spec = s.spec.as_ref();
    let name = spec.and_then(|sp| sp.name.clone()).unwrap_or_default();
    let mode = spec.and_then(|sp| sp.mode.as_ref());
    let (mode_str, replicas) = match mode {
        Some(m) if m.replicated.is_some() => (
            "replicated".to_string(),
            m.replicated
                .as_ref()
                .and_then(|r| r.replicas)
                .map(|n| n.to_string())
                .unwrap_or_default(),
        ),
        Some(m) if m.global.is_some() => ("global".to_string(), "-".to_string()),
        _ => (String::new(), String::new()),
    };
    let image = spec
        .and_then(|sp| sp.task_template.as_ref())
        .and_then(|tt| tt.container_spec.as_ref())
        .and_then(|cs| cs.image.clone())
        .map(|i| clean_image(&i))
        .unwrap_or_default();
    let ports = s
        .endpoint
        .as_ref()
        .and_then(|e| e.ports.as_ref())
        .map(|ps| {
            ps.iter()
                .filter_map(|p| {
                    p.published_port
                        .map(|pub_p| format!("{}:{}", pub_p, p.target_port.unwrap_or(0)))
                })
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let cells = vec![name.clone(), mode_str, replicas, image, ports];
    Item { id, name, cells, ..Default::default() }
}

fn fmt_ports(ports: &Option<Vec<bollard::models::PortSummary>>) -> String {
    let Some(ports) = ports else {
        return String::new();
    };
    let mut seen = Vec::new();
    for p in ports {
        let proto = opt_estr(&p.typ);
        let s = match p.public_port {
            Some(pub_p) => format!("{}->{}/{}", pub_p, p.private_port, proto),
            None => format!("{}/{}", p.private_port, proto),
        };
        if !seen.contains(&s) {
            seen.push(s);
        }
    }
    seen.join(", ")
}

// ---- swarm meta --------------------------------------------------------

/// Map volume name -> on-disk size (bytes) via the system df endpoint.
/// df returns volume items as untyped JSON, so we pluck Name / UsageData.Size.
pub async fn volume_sizes(docker: &Docker) -> std::collections::HashMap<String, i64> {
    let mut m = std::collections::HashMap::new();
    if let Ok(df) = docker.df(None::<DataUsageOptions>).await {
        if let Some(items) = df.volume_usage.and_then(|v| v.items) {
            for it in items {
                let name = it.get("Name").and_then(|v| v.as_str()).unwrap_or("");
                if name.is_empty() {
                    continue;
                }
                let size = it
                    .get("UsageData")
                    .and_then(|u| u.get("Size"))
                    .and_then(serde_json::Value::as_i64)
                    .unwrap_or(-1);
                m.insert(name.to_string(), size);
            }
        }
    }
    m
}

/// Map swarm service ids to (name, mode, declared-replicas) — for rebuilding
/// task names and the per-service header in the grouped stack-tasks view.
async fn service_meta(docker: &Docker) -> HashMap<String, (String, String, String)> {
    let mut m = HashMap::new();
    if let Ok(svcs) = docker.list_services(None::<ListServicesOptions>).await {
        for s in svcs {
            let id = s.id.clone().unwrap_or_default();
            if id.is_empty() {
                continue;
            }
            let spec = s.spec.as_ref();
            let name = spec.and_then(|sp| sp.name.clone()).unwrap_or_default();
            let mode = spec.and_then(|sp| sp.mode.as_ref());
            let (mode_str, declared) = match mode {
                Some(m) if m.replicated.is_some() => (
                    "replicated".to_string(),
                    m.replicated
                        .as_ref()
                        .and_then(|r| r.replicas)
                        .map(|n| n.to_string())
                        .unwrap_or_default(),
                ),
                Some(m) if m.global.is_some() => ("global".to_string(), String::new()),
                _ => (String::new(), String::new()),
            };
            m.insert(id, (name, mode_str, declared));
        }
    }
    m
}

/// Map container id -> its IP(s), for joining swarm tasks to overlay addresses.
/// Only containers on the connected daemon are visible, so tasks scheduled on
/// other swarm nodes come back without an IP (the column is left blank).
async fn container_ips(docker: &Docker) -> HashMap<String, String> {
    let mut m = HashMap::new();
    let opts = ListContainersOptionsBuilder::default().all(true).build();
    if let Ok(cs) = docker.list_containers(Some(opts)).await {
        for c in cs {
            let Some(id) = c.id else { continue };
            let ip = c
                .network_settings
                .as_ref()
                .and_then(|ns| ns.networks.as_ref())
                .map(|nets| {
                    let mut ips: Vec<String> = nets
                        .values()
                        .filter_map(|e| e.ip_address.clone())
                        .filter(|s| !s.is_empty())
                        .collect();
                    ips.sort();
                    ips.dedup();
                    ips.join(", ")
                })
                .unwrap_or_default();
            if !ip.is_empty() {
                m.insert(id, ip);
            }
        }
    }
    m
}

fn ipv4_to_u32(s: &str) -> Option<u32> {
    let mut it = s.split('.');
    let mut v: u32 = 0;
    for _ in 0..4 {
        let o: u32 = it.next()?.trim().parse().ok()?;
        if o > 255 {
            return None;
        }
        v = (v << 8) | o;
    }
    if it.next().is_some() {
        return None;
    }
    Some(v)
}

/// Parse an IPv4 CIDR "10.250.0.0/24" into (network_base, mask).
fn parse_cidr_v4(s: &str) -> Option<(u32, u32)> {
    let (ip, pfx) = s.split_once('/')?;
    let base = ipv4_to_u32(ip)?;
    let bits: u32 = pfx.trim().parse().ok()?;
    if bits > 32 {
        return None;
    }
    let mask = if bits == 0 { 0 } else { u32::MAX << (32 - bits) };
    Some((base & mask, mask))
}

fn ipv4_in_any(ip: &str, subnets: &[(u32, u32)]) -> bool {
    match ipv4_to_u32(ip) {
        Some(v) => subnets.iter().any(|&(base, mask)| (v & mask) == base),
        None => false,
    }
}

/// Subnets of the swarm ingress (routing-mesh) network(s). Used to drop the
/// ingress IP from a task's address list so we show the real overlay IP.
async fn ingress_subnets(docker: &Docker) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    if let Ok(nets) = docker.list_networks(None::<ListNetworksOptions>).await {
        for n in nets {
            let is_ingress = n.ingress == Some(true) || n.name.as_deref() == Some("ingress");
            if !is_ingress {
                continue;
            }
            if let Some(cfgs) = n.ipam.and_then(|i| i.config) {
                for c in cfgs {
                    if let Some(sn) = c.subnet.and_then(|s| parse_cidr_v4(&s)) {
                        out.push(sn);
                    }
                }
            }
        }
    }
    out
}

/// A task's overlay IP(s), read from the swarm manager's own record
/// (`NetworksAttachments`, assigned via IPAM). Works cluster-wide — no need to
/// reach the container's node. `ingress` subnets are dropped (routing mesh, not
/// the task's real IP). Bare IPs, no CIDR mask.
fn task_ip(t: &bollard::models::Task, ingress: &[(u32, u32)]) -> String {
    t.networks_attachments
        .as_ref()
        .map(|nas| {
            nas.iter()
                .flat_map(|na| na.addresses.clone().unwrap_or_default())
                .map(|a| a.split('/').next().unwrap_or(&a).to_string())
                .filter(|s| !s.is_empty() && !ipv4_in_any(s, ingress))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

/// Map swarm node ids to hostnames (for the service-tasks NODE column).
async fn node_hostnames(docker: &Docker) -> HashMap<String, String> {
    let mut m = HashMap::new();
    if let Ok(nodes) = docker.list_nodes(None::<ListNodesOptions>).await {
        for n in nodes {
            if let (Some(id), Some(host)) = (n.id, n.description.and_then(|d| d.hostname)) {
                m.insert(id, host);
            }
        }
    }
    m
}

pub async fn swarm_state(docker: &Docker) -> String {
    match docker.info().await {
        Ok(info) => info
            .swarm
            .and_then(|s| s.local_node_state)
            .map(|st| estr(&st))
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "inactive".into()),
        Err(_) => "unreachable".into(),
    }
}

// ---- inspect -----------------------------------------------------------

pub async fn inspect(docker: &Docker, kind: &str, id: &str) -> Result<String> {
    let json = match kind {
        "container" => serde_json::to_string_pretty(
            &docker
                .inspect_container(id, None::<InspectContainerOptions>)
                .await?,
        )?,
        "image" => serde_json::to_string_pretty(&docker.inspect_image(id).await?)?,
        "service" => serde_json::to_string_pretty(
            &docker
                .inspect_service(id, None::<InspectServiceOptions>)
                .await?,
        )?,
        "node" => serde_json::to_string_pretty(&docker.inspect_node(id).await?)?,
        "volume" => serde_json::to_string_pretty(&docker.inspect_volume(id).await?)?,
        "network" => serde_json::to_string_pretty(
            &docker
                .inspect_network(id, None::<InspectNetworkOptions>)
                .await?,
        )?,
        other => return Err(anyhow!("cannot inspect '{other}'")),
    };
    Ok(json)
}

// ---- lifecycle actions -------------------------------------------------

pub async fn container_action(docker: &Docker, verb: &str, id: &str) -> Result<()> {
    match verb {
        "start" => {
            docker
                .start_container(id, None::<StartContainerOptions>)
                .await?
        }
        "stop" => {
            docker
                .stop_container(id, None::<StopContainerOptions>)
                .await?
        }
        "restart" => {
            docker
                .restart_container(id, None::<RestartContainerOptions>)
                .await?
        }
        "pause" => docker.pause_container(id).await?,
        "unpause" => docker.unpause_container(id).await?,
        "rm" => {
            let opts = RemoveContainerOptionsBuilder::default().force(true).build();
            docker.remove_container(id, Some(opts)).await?
        }
        other => return Err(anyhow!("unknown action '{other}'")),
    }
    Ok(())
}

// ---- log streaming -----------------------------------------------------

fn fmt_log(item: Result<bollard::container::LogOutput, bollard::errors::Error>) -> String {
    match item {
        Ok(out) => out.to_string().trim_end_matches(['\n', '\r']).to_string(),
        Err(e) => format!("⚠ {e}"),
    }
}

/// A boxed stream of already-formatted log lines for a container or service.
pub fn log_stream(docker: &Docker, view: View, id: &str, tail: u32) -> BoxStream<'static, String> {
    let tail = tail.to_string();
    let opts = LogsOptionsBuilder::default()
        .follow(true)
        .stdout(true)
        .stderr(true)
        .tail(tail.as_str())
        .timestamps(true)
        .build();
    match view {
        // service_logs also accepts LogsOptions in bollard 0.21
        View::Services => docker.service_logs(id, Some(opts)).map(fmt_log).boxed(),
        _ => docker.logs(id, Some(opts)).map(fmt_log).boxed(),
    }
}

// ---- live container stats ----------------------------------------------

/// A computed one-shot sample of a container's resource usage.
#[derive(Clone, Debug, Default)]
pub struct StatsSample {
    pub cpu_pct: f64,
    pub mem_used: u64,
    pub mem_limit: u64,
    pub mem_pct: f64,
    pub net_rx: u64,
    pub net_tx: u64,
    pub blk_r: u64,
    pub blk_w: u64,
    pub pids: u64,
}

/// A stream of computed stats samples for a container (~1/sec from dockerd).
pub fn stats_stream(docker: &Docker, id: &str) -> BoxStream<'static, StatsSample> {
    let opts = StatsOptionsBuilder::default().stream(true).build();
    docker
        .stats(id, Some(opts))
        .filter_map(|r| futures::future::ready(r.ok().map(|s| compute_stats(&s))))
        .boxed()
}

fn compute_stats(s: &bollard::models::ContainerStatsResponse) -> StatsSample {
    let mut out = StatsSample::default();

    // CPU%: delta of container cpu usage over delta of system cpu usage, x nCPU.
    // precpu_stats is the previous sample (populated from the 2nd message on).
    if let (Some(cpu), Some(pre)) = (&s.cpu_stats, &s.precpu_stats) {
        let cur = cpu
            .cpu_usage
            .as_ref()
            .and_then(|u| u.total_usage)
            .unwrap_or(0);
        let prev = pre
            .cpu_usage
            .as_ref()
            .and_then(|u| u.total_usage)
            .unwrap_or(0);
        let cur_sys = cpu.system_cpu_usage.unwrap_or(0);
        let pre_sys = pre.system_cpu_usage.unwrap_or(0);
        let cpu_delta = cur.saturating_sub(prev) as f64;
        let sys_delta = cur_sys.saturating_sub(pre_sys) as f64;
        let ncpu = cpu
            .online_cpus
            .or_else(|| {
                cpu.cpu_usage
                    .as_ref()
                    .and_then(|u| u.percpu_usage.as_ref().map(|v| v.len() as u32))
            })
            .unwrap_or(1)
            .max(1) as f64;
        if sys_delta > 0.0 && prev > 0 {
            out.cpu_pct = (cpu_delta / sys_delta) * ncpu * 100.0;
        }
    }

    // Memory: usage minus page cache, against the limit (like `docker stats`).
    if let Some(mem) = &s.memory_stats {
        let usage = mem.usage.unwrap_or(0);
        let cache = mem
            .stats
            .as_ref()
            .and_then(|m| m.get("inactive_file").or_else(|| m.get("cache")).copied())
            .unwrap_or(0);
        out.mem_used = usage.saturating_sub(cache);
        out.mem_limit = mem.limit.unwrap_or(0);
        if out.mem_limit > 0 {
            out.mem_pct = out.mem_used as f64 / out.mem_limit as f64 * 100.0;
        }
    }

    // Network: sum over all interfaces.
    if let Some(nets) = &s.networks {
        for n in nets.values() {
            out.net_rx += n.rx_bytes.unwrap_or(0);
            out.net_tx += n.tx_bytes.unwrap_or(0);
        }
    }

    // Block IO: sum service-bytes by op.
    if let Some(blk) = &s.blkio_stats {
        if let Some(entries) = &blk.io_service_bytes_recursive {
            for e in entries {
                let v = e.value.unwrap_or(0);
                match e.op.as_deref().unwrap_or("").to_lowercase().as_str() {
                    "read" => out.blk_r += v,
                    "write" => out.blk_w += v,
                    _ => {}
                }
            }
        }
    }

    out.pids = s.pids_stats.as_ref().and_then(|p| p.current).unwrap_or(0);
    out
}

// ---- resource deletion / swarm scale / prune ---------------------------

/// Delete the selected resource for a view (force where applicable).
pub async fn delete(docker: &Docker, view: View, id: &str) -> Result<()> {
    match view {
        View::Containers => {
            let opts = RemoveContainerOptionsBuilder::default().force(true).build();
            docker.remove_container(id, Some(opts)).await?;
        }
        View::Images => {
            let opts = RemoveImageOptionsBuilder::default().force(true).build();
            docker.remove_image(id, Some(opts), None).await?;
        }
        View::Volumes => {
            let opts = RemoveVolumeOptionsBuilder::default().force(true).build();
            docker.remove_volume(id, Some(opts)).await?;
        }
        View::Networks => docker.remove_network(id).await?,
        _ => return Err(anyhow!("cannot delete from this view")),
    }
    Ok(())
}

/// Scale a replicated swarm service by `delta` replicas.
pub async fn scale_service(docker: &Docker, name: &str, delta: i64) -> Result<String> {
    let svc = docker
        .inspect_service(name, None::<InspectServiceOptions>)
        .await?;
    let version = svc
        .version
        .and_then(|v| v.index)
        .ok_or_else(|| anyhow!("service has no version"))? as i32;
    let mut spec = svc.spec.ok_or_else(|| anyhow!("service has no spec"))?;
    let repl = spec
        .mode
        .as_mut()
        .and_then(|m| m.replicated.as_mut())
        .ok_or_else(|| anyhow!("not a replicated service"))?;
    let cur = repl.replicas.unwrap_or(0);
    let next = (cur + delta).max(0);
    repl.replicas = Some(next);
    let opts = UpdateServiceOptionsBuilder::default()
        .version(version)
        .build();
    docker.update_service(name, spec, opts, None).await?;
    Ok(format!("scaled {name}: {cur} -> {next}"))
}

/// Prune dangling images; returns a human summary.
pub async fn prune_images(docker: &Docker) -> Result<String> {
    let resp = docker.prune_images(None::<PruneImagesOptions>).await?;
    let n = resp.images_deleted.map(|v| v.len()).unwrap_or(0);
    let reclaimed = resp.space_reclaimed.unwrap_or(0);
    Ok(format!(
        "pruned {n} image(s), reclaimed {}",
        human_size(reclaimed)
    ))
}

/// ≈ `docker system prune -a` without `--volumes`: unused containers, networks,
/// unused images (not just dangling), and build cache.
pub async fn system_prune_all(docker: &Docker) -> Result<String> {
    let c = docker
        .prune_containers(None::<PruneContainersOptions>)
        .await?;
    let n_c = c.containers_deleted.as_ref().map(|v| v.len()).unwrap_or(0);
    let reclaim_c = c.space_reclaimed.unwrap_or(0);

    let net = docker.prune_networks(None::<PruneNetworksOptions>).await?;
    let n_net = net.networks_deleted.as_ref().map(|v| v.len()).unwrap_or(0);

    let mut img_filters = HashMap::new();
    img_filters.insert("dangling", vec!["false"]);
    let img = docker
        .prune_images(Some(
            PruneImagesOptionsBuilder::default()
                .filters(&img_filters)
                .build(),
        ))
        .await?;
    let n_img = img.images_deleted.as_ref().map(|v| v.len()).unwrap_or(0);
    let reclaim_img = img.space_reclaimed.unwrap_or(0);

    let build = docker.prune_build(None::<PruneBuildOptions>).await?;
    let n_build = build.caches_deleted.as_ref().map(|v| v.len()).unwrap_or(0);
    let reclaim_build = build.space_reclaimed.unwrap_or(0);

    let reclaimed = reclaim_c + reclaim_img + reclaim_build;
    Ok(format!(
        "system prune -a: {n_c} container(s), {n_net} network(s), {n_img} image(s), {n_build} build cache; reclaimed {} (volumes NOT pruned)",
        human_size(reclaimed)
    ))
}

/// Number of distinct compose projects on this engine (0 => hide the Compose tab).
pub async fn compose_count(docker: &Docker) -> usize {
    let opts = ListContainersOptionsBuilder::default().all(true).build();
    let Ok(cs) = docker.list_containers(Some(opts)).await else {
        return 0;
    };
    let mut set = std::collections::HashSet::new();
    for c in cs {
        if let Some(l) = c.labels {
            if let Some(p) = l.get("com.docker.compose.project") {
                if !p.is_empty() {
                    set.insert(p.clone());
                }
            }
        }
    }
    set.len()
}
