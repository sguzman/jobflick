//! Resolve requested graphical/GPU execution contexts for a single child job.
//! This never changes the daemon's own process environment.
use anyhow::{bail, Context, Result};
use execarium::v3::{Environment, Gpu, Session};
use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Debug, Default)]
pub(crate) struct ChildContext {
    set: Vec<(OsString, OsString)>,
    remove: Vec<&'static str>,
}

impl ChildContext {
    fn set(&mut self, name: &'static str, value: impl Into<OsString>) {
        self.set.push((OsString::from(name), value.into()));
    }

    pub(crate) fn apply(&self, command: &mut Command) {
        for key in &self.remove {
            command.env_remove(key);
        }
        for (key, value) in &self.set {
            command.env(key, value);
        }
    }
}

fn manager_vars() -> HashMap<String, String> {
    // When started by systemd, the daemon may not inherit the graphical
    // compositor environment. Query the *current* user-manager environment.
    let mut values = HashMap::new();
    if let Ok(output) = Command::new("systemctl")
        .args(["--user", "show-environment"])
        .stdin(Stdio::null())
        .output()
    {
        if output.status.success() {
            for line in String::from_utf8_lossy(&output.stdout).lines() {
                if let Some((key, value)) = line.split_once('=') {
                    if matches!(
                        key,
                        "WAYLAND_DISPLAY"
                            | "HYPRLAND_INSTANCE_SIGNATURE"
                            | "XDG_RUNTIME_DIR"
                            | "DBUS_SESSION_BUS_ADDRESS"
                    ) {
                        values.insert(key.to_owned(), value.to_owned());
                    }
                }
            }
        }
    }
    values
}

fn runtime_dir(manager: &HashMap<String, String>) -> Result<PathBuf> {
    let uid = unsafe { libc::geteuid() };
    // The per-user runtime directory is stable across graphical sessions.
    // Unlike WAYLAND_DISPLAY it is generally safe to inherit from the daemon.
    let path = env::var_os("XDG_RUNTIME_DIR")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| manager.get("XDG_RUNTIME_DIR").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(format!("/run/user/{uid}")));
    if !path.is_absolute() || !path.is_dir() {
        bail!(
            "Graphical context unavailable: XDG_RUNTIME_DIR is not an accessible directory ({})",
            path.display()
        );
    }
    Ok(path)
}

fn is_socket(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.file_type().is_socket())
        .unwrap_or(false)
}

fn wayland_socket(runtime: &Path, name: &str) -> Option<PathBuf> {
    if name.is_empty() || name.contains('\0') {
        return None;
    }
    let path = Path::new(name);
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else if path.components().count() == 1 {
        runtime.join(path)
    } else {
        return None;
    };
    is_socket(&candidate).then_some(candidate)
}

fn socket_candidates(runtime: &Path) -> Vec<String> {
    let mut found = Vec::new();
    if let Ok(entries) = fs::read_dir(runtime) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with("wayland-") && !name.ends_with(".lock") && is_socket(&entry.path())
            {
                found.push(name.to_owned());
            }
        }
    }
    found.sort();
    found
}

fn select_wayland(
    runtime: &Path,
    manager: &HashMap<String, String>,
    inherited: &HashMap<String, String>,
) -> Result<String> {
    // Prefer a live socket explicitly published by the graphical session.
    for source in [manager, inherited] {
        if let Some(value) = source.get("WAYLAND_DISPLAY") {
            if wayland_socket(runtime, value).is_some() {
                return Ok(value.clone());
            }
        }
    }
    let found = socket_candidates(runtime);
    match found.as_slice() {
        [only] => Ok(only.clone()),
        [] => bail!("Wayland context unavailable: no active Wayland socket in {}", runtime.display()),
        _ => bail!("Wayland context ambiguous: multiple sockets; export WAYLAND_DISPLAY to the systemd user manager"),
    }
}

fn is_live_hypr(runtime: &Path, signature: &str) -> bool {
    if signature.is_empty() || signature.contains('/') || signature == "." || signature == ".." {
        return false;
    }
    let root = runtime.join("hypr").join(signature);
    is_socket(&root.join(".socket.sock")) || is_socket(&root.join(".socket2.sock"))
}

fn select_hyprland(
    runtime: &Path,
    manager: &HashMap<String, String>,
    inherited: &HashMap<String, String>,
) -> Result<String> {
    for source in [manager, inherited] {
        if let Some(value) = source.get("HYPRLAND_INSTANCE_SIGNATURE") {
            if is_live_hypr(runtime, value) {
                return Ok(value.clone());
            }
        }
    }
    let mut found = Vec::new();
    if let Ok(entries) = fs::read_dir(runtime.join("hypr")) {
        for entry in entries.flatten() {
            if let Some(sig) = entry.file_name().to_str() {
                if is_live_hypr(runtime, sig) {
                    found.push(sig.to_owned());
                }
            }
        }
    }
    found.sort();
    match found.as_slice() {
        [only] => Ok(only.clone()),
        [] => bail!("Hyprland context unavailable: no live compositor IPC socket in {}/hypr", runtime.display()),
        _ => bail!("Hyprland context ambiguous: multiple instances; export HYPRLAND_INSTANCE_SIGNATURE to the systemd user manager"),
    }
}

fn render_vendor_present(vendor: &str) -> bool {
    let Ok(entries) = fs::read_dir("/sys/class/drm") else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        name.starts_with("renderD")
            && fs::read_to_string(entry.path().join("device/vendor"))
                .map(|value| value.trim().eq_ignore_ascii_case(vendor))
                .unwrap_or(false)
    })
}

fn nvidia_available() -> bool {
    Path::new("/dev/nvidiactl").exists()
        || Path::new("/proc/driver/nvidia/gpus").is_dir()
        || render_vendor_present("0x10de")
}

fn integrated_available() -> bool {
    render_vendor_present("0x8086") || render_vendor_present("0x1002")
}

fn configure_gpu(gpu: Gpu, context: &mut ChildContext) -> Result<()> {
    match gpu {
        Gpu::Inherit => {}
        Gpu::Integrated => {
            if !integrated_available() {
                bail!("Integrated GPU context unavailable: no Intel/AMD render device discovered");
            }
            context
                .remove
                .extend(["__GLX_VENDOR_LIBRARY_NAME", "__VK_LAYER_NV_optimus"]);
            context.set("__NV_PRIME_RENDER_OFFLOAD", "0");
            context.set("DRI_PRIME", "0");
        }
        Gpu::Nvidia => {
            if !nvidia_available() {
                bail!("NVIDIA GPU context unavailable: no NVIDIA device discovered");
            }
            context.remove.push("DRI_PRIME");
            context.set("__NV_PRIME_RENDER_OFFLOAD", "1");
            context.set("__GLX_VENDOR_LIBRARY_NAME", "nvidia");
            context.set("__VK_LAYER_NV_optimus", "NVIDIA_only");
            // CUDA-aware processes use the NVIDIA driver independently of
            // Wayland. Avoid setting CUDA_VISIBLE_DEVICES because device
            // enumeration and container setups differ.
        }
    }
    Ok(())
}

pub(crate) fn resolve(environment: Environment) -> Result<ChildContext> {
    let mut context = ChildContext::default();
    if environment.session() != Session::Inherit {
        let manager = manager_vars();
        let inherited: HashMap<String, String> = env::vars()
            .filter(|(key, _)| {
                matches!(
                    key.as_str(),
                    "WAYLAND_DISPLAY" | "HYPRLAND_INSTANCE_SIGNATURE" | "DBUS_SESSION_BUS_ADDRESS"
                )
            })
            .collect();
        let runtime = runtime_dir(&manager)?;
        let wayland = select_wayland(&runtime, &manager, &inherited)?;
        context.set("XDG_RUNTIME_DIR", runtime.as_os_str().to_os_string());
        context.set("WAYLAND_DISPLAY", wayland);
        if environment.session() == Session::Hyprland {
            context.set(
                "HYPRLAND_INSTANCE_SIGNATURE",
                select_hyprland(&runtime, &manager, &inherited)?,
            );
        } else {
            // An inherited stale compositor ID should not identify the
            // requested generic Wayland session.
            context.remove.push("HYPRLAND_INSTANCE_SIGNATURE");
        }
        if let Some(bus) = manager.get("DBUS_SESSION_BUS_ADDRESS") {
            context.set("DBUS_SESSION_BUS_ADDRESS", bus.as_str());
        } else if runtime.join("bus").exists() {
            context.set(
                "DBUS_SESSION_BUS_ADDRESS",
                format!("unix:path={}/bus", runtime.display()),
            );
        }
    }
    configure_gpu(environment.gpu(), &mut context)
        .context("cannot satisfy requested GPU execution context")?;
    Ok(context)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use uuid::Uuid;

    #[test]
    fn selects_only_live_and_unambiguous_wayland_sockets() {
        let root = env::temp_dir().join(format!("jobflick-wayland-{}", Uuid::new_v4()));
        fs::create_dir(&root).unwrap();
        let _first = UnixListener::bind(root.join("wayland-1")).unwrap();
        let mut settings = HashMap::new();
        assert_eq!(
            select_wayland(&root, &settings, &settings).unwrap(),
            "wayland-1"
        );
        let _second = UnixListener::bind(root.join("wayland-2")).unwrap();
        assert!(select_wayland(&root, &settings, &settings).is_err());
        settings.insert("WAYLAND_DISPLAY".into(), "wayland-2".into());
        assert_eq!(
            select_wayland(&root, &settings, &HashMap::new()).unwrap(),
            "wayland-2"
        );
        settings.insert("WAYLAND_DISPLAY".into(), "wayland-404".into());
        assert!(select_wayland(&root, &settings, &HashMap::new()).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hyprland_needs_a_real_runtime_socket() {
        let root = env::temp_dir().join(format!("jobflick-hypr-{}", Uuid::new_v4()));
        let dir = root.join("hypr").join("fake-instance");
        fs::create_dir_all(&dir).unwrap();
        let vars = HashMap::new();
        assert!(select_hyprland(&root, &vars, &vars).is_err());
        let _socket = UnixListener::bind(dir.join(".socket2.sock")).unwrap();
        assert_eq!(
            select_hyprland(&root, &vars, &vars).unwrap(),
            "fake-instance"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn inherited_environment_has_no_overrides() {
        let context = resolve(Environment::new(Session::Inherit, Gpu::Inherit)).unwrap();
        assert!(context.set.is_empty());
        assert!(context.remove.is_empty());
    }

    #[test]
    fn applies_overrides_to_child_not_parent() {
        let mut context = ChildContext::default();
        context.set("WAYLAND_DISPLAY", "wayland-test");
        context.remove.push("HYPRLAND_INSTANCE_SIGNATURE");
        let mut command = Command::new("env");
        context.apply(&mut command);
        let result = command
            .env("HYPRLAND_INSTANCE_SIGNATURE", "stale")
            .output()
            .unwrap();
        // env() after apply() intentionally overrides our removal: in the
        // worker the child overrides are applied last.
        let content = String::from_utf8(result.stdout).unwrap();
        assert!(content.contains("WAYLAND_DISPLAY=wayland-test"));
    }
}
