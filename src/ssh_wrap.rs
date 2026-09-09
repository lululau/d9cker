//! Workaround for bollard/openssh + user `ControlMaster yes`.
//!
//! bollard's SSH transport (via the `openssh` crate) runs remote commands as:
//!
//! ```text
//! ssh -S <ctl> -T -p 9 none -- docker system dial-stdio
//! ```
//!
//! The dummy destination `none:9` is intentional: mux should go through the
//! control socket, and a real TCP connect to port 9 should never succeed.
//!
//! But with `Host * / ControlMaster yes` in `~/.ssh/config` (common for
//! connection sharing), OpenSSH 10 treats the mux client as a would-be master,
//! sees the socket already exists, **disables multiplexing**, and falls back to
//! a real TCP connect to host `none` port 9.
//!
//! Under Clash/fake-ip DNS, `none` resolves to something like `198.18.x.x`,
//! which accepts then closes — surfacing as hyper `SendRequest` errors.
//!
//! Fix: put a tiny `ssh` wrapper first on `PATH` that injects
//! `-o ControlMaster=no` for mux-client invocations (`-S` without `-M`).

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::Once;

static INSTALL: Once = Once::new();

const WRAPPER: &str = r#"#!/bin/sh
# d9cker: force ControlMaster=no for openssh-crate mux clients.
real_ssh="${D9CKER_REAL_SSH:-/usr/bin/ssh}"
has_S=0
has_M=0
for a in "$@"; do
  if [ "$a" = "-S" ]; then has_S=1; fi
  if [ "$a" = "-M" ]; then has_M=1; fi
done
if [ "$has_S" -eq 1 ] && [ "$has_M" -eq 0 ]; then
  exec "$real_ssh" -o ControlMaster=no "$@"
fi
exec "$real_ssh" "$@"
"#;

/// Install the mux-compat ssh wrapper on `PATH` (once per process).
pub fn install() {
    INSTALL.call_once(|| {
        if let Err(e) = install_inner() {
            eprintln!("d9cker: warning: ssh mux wrapper not installed: {e}");
        }
    });
}

fn install_inner() -> std::io::Result<()> {
    let real = resolve_real_ssh();
    // SAFETY: called once before any bollard ssh spawn; child processes inherit.
    unsafe {
        std::env::set_var("D9CKER_REAL_SSH", &real);
    }

    let dir = wrap_dir()?;
    fs::create_dir_all(&dir)?;
    let path = dir.join("ssh");
    {
        let mut f = fs::File::create(&path)?;
        f.write_all(WRAPPER.as_bytes())?;
        f.sync_all()?;
    }
    let mut perms = fs::metadata(&path)?.permissions();
    perms.set_mode(0o755);
    fs::set_permissions(&path, perms)?;

    let path_env = std::env::var_os("PATH").unwrap_or_default();
    let already = std::env::split_paths(&path_env).any(|p| p == dir);
    if !already {
        let mut paths = vec![dir];
        paths.extend(std::env::split_paths(&path_env));
        let joined = std::env::join_paths(paths).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;
        // SAFETY: same as above — once, before ssh children.
        unsafe {
            std::env::set_var("PATH", joined);
        }
    }
    Ok(())
}

fn wrap_dir() -> std::io::Result<PathBuf> {
    if let Ok(cache) = std::env::var("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(cache).join("d9cker").join("bin"));
    }
    if let Ok(home) = std::env::var("HOME") {
        // macOS-friendly default when XDG_CACHE_HOME is unset
        let mac = PathBuf::from(&home).join("Library/Caches/d9cker/bin");
        if cfg!(target_os = "macos") {
            return Ok(mac);
        }
        return Ok(PathBuf::from(home).join(".cache/d9cker/bin"));
    }
    Ok(std::env::temp_dir().join("d9cker-ssh-wrap"))
}

fn resolve_real_ssh() -> PathBuf {
    // Prefer a real binary that isn't our own wrapper (PATH may already include it
    // from a previous run in the same shell — uncommon but possible).
    for candidate in ["/usr/bin/ssh", "/bin/ssh"] {
        let p = PathBuf::from(candidate);
        if p.is_file() {
            return p;
        }
    }
    PathBuf::from("ssh")
}
