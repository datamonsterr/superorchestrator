//! `ui [--bind] [--port] [--token] [--install-service]`.
//!
//! The web server runs inside the supervisor. `ui` starts the supervisor if needed, asks it
//! to serve the UI (`ui_start`), prints the URL and opens the browser. `--install-service`
//! instead writes a systemd user unit or launchd agent that runs `daemon --foreground`, and
//! turns on `ui.enabled` so that supervisor serves the UI at start.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};
use serde_json::json;
use so_core::proto::{METHOD_UI_START, UiStartParams, UiStartResult};
use so_core::{Config, Platform};

use super::runs::{connect, runtime};
use crate::cli::UiArgs;
use crate::ctx::Ctx;

pub const SYSTEMD_UNIT: &str = ".config/systemd/user/superorchestrator.service";
pub const LAUNCHD_PLIST: &str = "Library/LaunchAgents/dev.superorchestrator.plist";

pub fn run(ctx: &Ctx, args: UiArgs) -> Result<()> {
    if args.install_service {
        return install_service(ctx, &args);
    }
    let params = UiStartParams {
        bind: args.bind.clone(),
        port: args.port,
        token: args.token.clone(),
    };
    let result: UiStartResult = runtime()?.block_on(async {
        let client = connect(ctx).await?;
        client
            .call(METHOD_UI_START, params)
            .await
            .map_err(|e| anyhow::anyhow!("could not start the UI: {e}"))
    })?;
    let opened = !args.no_open && open_browser(&result.url);
    ctx.emit(
        &json!({"ok": true, "url": result.url, "already_running": result.already_running, "opened": opened}),
        |w| {
            writeln!(w, "web UI: {}", result.url)?;
            if result.already_running && (args.bind.is_some() || args.port.is_some() || args.token.is_some()) {
                writeln!(w, "note: the UI was already running; --bind, --port and --token were ignored")?;
            }
            Ok(())
        },
    )
}

/// Opens `url` with `xdg-open` (Linux, only with a display) or `open` (macOS).
fn open_browser(url: &str) -> bool {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        if std::env::var_os("DISPLAY").is_none() && std::env::var_os("WAYLAND_DISPLAY").is_none() {
            return false;
        }
        "xdg-open"
    };
    Command::new(program)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

fn quote_systemd(arg: &str) -> String {
    // `%` starts a specifier in unit files, so it is doubled.
    let arg = arg.replace('%', "%%");
    if arg.contains([' ', '"', '\\']) {
        format!("\"{}\"", arg.replace('\\', "\\\\").replace('"', "\\\""))
    } else {
        arg
    }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The command the service runs.
fn service_argv(exe: &Path, config_dir: &Path) -> Vec<String> {
    vec![
        exe.to_string_lossy().into_owned(),
        "--config-dir".into(),
        config_dir.to_string_lossy().into_owned(),
        "daemon".into(),
        "--foreground".into(),
    ]
}

pub fn systemd_unit(argv: &[String]) -> String {
    let exec = argv
        .iter()
        .map(|a| quote_systemd(a))
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "[Unit]\nDescription=Superorchestrator supervisor and web UI\nAfter=network.target\n\n\
         [Service]\nExecStart={exec}\nRestart=always\nRestartSec=5\n\n\
         [Install]\nWantedBy=default.target\n"
    )
}

pub fn launchd_plist(argv: &[String], log: &Path) -> String {
    let args: String = argv
        .iter()
        .map(|a| format!("    <string>{}</string>\n", xml_escape(a)))
        .collect();
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n<dict>\n  <key>Label</key>\n  <string>dev.superorchestrator</string>\n\
         \x20 <key>ProgramArguments</key>\n  <array>\n{args}  </array>\n\
         \x20 <key>RunAtLoad</key>\n  <true/>\n  <key>KeepAlive</key>\n  <true/>\n\
         \x20 <key>StandardErrorPath</key>\n  <string>{}</string>\n</dict>\n</plist>\n",
        xml_escape(&log.to_string_lossy())
    )
}

fn install_service(ctx: &Ctx, args: &UiArgs) -> Result<()> {
    let host = ctx.host()?;
    let exe = std::env::current_exe().context("could not find this executable")?;
    let argv = service_argv(&exe, &ctx.paths.config_dir);
    let (path, text, enable): (PathBuf, String, String) = match host.platform {
        Platform::Linux => {
            let p = host.home.join(SYSTEMD_UNIT);
            (
                p,
                systemd_unit(&argv),
                "systemctl --user daemon-reload && systemctl --user enable --now superorchestrator.service".into(),
            )
        }
        Platform::MacOs => {
            let p = host.home.join(LAUNCHD_PLIST);
            let log = ctx.paths.logs_dir().join("supervisor.log");
            let enable = format!("launchctl load -w {}", p.display());
            (p, launchd_plist(&argv, &log), enable)
        }
    };
    std::fs::create_dir_all(path.parent().context("no parent directory")?)
        .with_context(|| format!("could not create the directory of {}", path.display()))?;
    std::fs::write(&path, text).with_context(|| format!("could not write {}", path.display()))?;

    // The service runs the plain supervisor, so the UI must be switched on in config.
    Config::set_in_file(&ctx.paths, "ui.enabled", "true")?;
    if let Some(b) = &args.bind {
        Config::set_in_file(&ctx.paths, "ui.bind", &format!("\"{b}\""))?;
    }
    if let Some(p) = args.port {
        Config::set_in_file(&ctx.paths, "ui.port", &p.to_string())?;
    }
    if let Some(t) = &args.token {
        Config::set_in_file(&ctx.paths, "ui.token", &json!(t).to_string())?;
    }
    ctx.emit(
        &json!({"ok": true, "service": path, "enable": enable}),
        |w| {
            writeln!(w, "wrote {}", path.display())?;
            writeln!(
                w,
                "set ui.enabled = true in {}",
                ctx.paths.config_toml().display()
            )?;
            writeln!(w, "enable it with:\n  {enable}")
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_and_plist_run_the_foreground_daemon() {
        let argv = service_argv(Path::new("/opt/my app/so"), Path::new("/home/u/.config/so"));
        let unit = systemd_unit(&argv);
        assert!(unit.contains(
            "ExecStart=\"/opt/my app/so\" --config-dir /home/u/.config/so daemon --foreground"
        ));
        assert!(unit.contains("WantedBy=default.target"));
        assert_eq!(quote_systemd("/opt/100%/so"), "/opt/100%%/so");
        let plist = launchd_plist(&argv, Path::new("/tmp/a&b.log"));
        assert!(plist.contains("<string>--foreground</string>") && plist.contains("a&amp;b.log"));
    }
}
