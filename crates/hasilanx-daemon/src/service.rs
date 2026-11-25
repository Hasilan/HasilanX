use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[cfg(target_os = "linux")]
fn service_unit_path() -> Result<PathBuf> {
    let base = dirs::config_dir().context("cannot resolve ~/.config")?;
    Ok(base.join("systemd/user/hasilanx-daemon.service"))
}

#[cfg(target_os = "macos")]
fn service_plist_path() -> Result<PathBuf> {
    let home = dirs::home_dir().context("cannot resolve home directory")?;
    Ok(home.join("Library/LaunchAgents/dev.hasilanx.daemon.plist"))
}

pub fn install(config_path: Option<&std::path::Path>) -> Result<String> {
    let exe = std::env::current_exe().context("cannot determine hasilanx-daemon executable path")?;
    let exe_str = exe.to_string_lossy().to_string();

    #[cfg(target_os = "linux")]
    {
        let unit_path = service_unit_path()?;
        if let Some(parent) = unit_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let exec_start = match config_path {
            Some(p) => format!("{exe_str} run --config {}", p.display()),
            None => format!("{exe_str} run"),
        };
        let unit = format!(
            "[Unit]\n\
             Description=HasilanX AI Coding Agent Control Plane Daemon\n\
             After=network-online.target\n\
             \n\
             [Service]\n\
             Type=simple\n\
             ExecStart={exec_start}\n\
             Restart=on-failure\n\
             RestartSec=3\n\
             Environment=PATH=/usr/local/bin:/usr/bin:/bin:%h/.local/bin:%h/.cargo/bin\n\
             \n\
             [Install]\n\
             WantedBy=default.target\n"
        );
        fs::write(&unit_path, unit)?;
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
        let st = Command::new("systemctl")
            .args(["--user", "enable", "--now", "hasilanx-daemon.service"])
            .status();
        let enabled_note = match st {
            Ok(s) if s.success() => "enabled and started via systemd --user",
            _ => "unit file written (run `systemctl --user enable --now hasilanx-daemon.service` in a user systemd session)",
        };
        return Ok(format!(
            "Installed systemd user unit at {} ({enabled_note})",
            unit_path.display()
        ));
    }

    #[cfg(target_os = "macos")]
    {
        let plist_path = service_plist_path()?;
        if let Some(parent) = plist_path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut args_xml = format!("    <string>{exe_str}</string>\n    <string>run</string>\n");
        if let Some(p) = config_path {
            args_xml.push_str(&format!(
                "    <string>--config</string>\n    <string>{}</string>\n",
                p.display()
            ));
        }
        let plist = format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n\
             <dict>\n\
               <key>Label</key><string>dev.hasilanx.daemon</string>\n\
               <key>ProgramArguments</key>\n\
               <array>\n{args_xml}  </array>\n\
               <key>RunAtLoad</key><true/>\n\
               <key>KeepAlive</key><true/>\n\
             </dict>\n\
             </plist>\n"
        );
        fs::write(&plist_path, plist)?;
        let _ = Command::new("launchctl")
            .args(["load", "-w", &plist_path.to_string_lossy()])
            .status();
        return Ok(format!(
            "Installed launchd agent at {}",
            plist_path.display()
        ));
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = (exe_str, config_path);
        Err(anyhow!("service install is supported on Linux and macOS"))
    }
}

pub fn uninstall() -> Result<String> {
    #[cfg(target_os = "linux")]
    {
        let unit_path = service_unit_path()?;
        let _ = Command::new("systemctl")
            .args(["--user", "disable", "--now", "hasilanx-daemon.service"])
            .status();
        if unit_path.exists() {
            fs::remove_file(&unit_path)?;
        }
        let _ = Command::new("systemctl")
            .args(["--user", "daemon-reload"])
            .status();
        return Ok(format!("Removed systemd unit {}", unit_path.display()));
    }

    #[cfg(target_os = "macos")]
    {
        let plist_path = service_plist_path()?;
        if plist_path.exists() {
            let _ = Command::new("launchctl")
                .args(["unload", "-w", &plist_path.to_string_lossy()])
                .status();
            fs::remove_file(&plist_path)?;
        }
        return Ok(format!("Removed launchd plist {}", plist_path.display()));
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Err(anyhow!("service uninstall is supported on Linux and macOS"))
    }
}

pub fn status() -> Result<String> {
    #[cfg(target_os = "linux")]
    {
        let unit_path = service_unit_path()?;
        if !unit_path.exists() {
            return Ok(format!("Not installed ({} does not exist)", unit_path.display()));
        }
        let out = Command::new("systemctl")
            .args(["--user", "is-active", "hasilanx-daemon.service"])
            .output();
        let active = out
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
            .unwrap_or_else(|_| "unknown".to_string());
        return Ok(format!(
            "Installed at {} (systemd status: {active})",
            unit_path.display()
        ));
    }

    #[cfg(target_os = "macos")]
    {
        let plist_path = service_plist_path()?;
        if !plist_path.exists() {
            return Ok(format!("Not installed ({} does not exist)", plist_path.display()));
        }
        return Ok(format!("Installed at {}", plist_path.display()));
    }

    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        Err(anyhow!("service status is supported on Linux and macOS"))
    }
}
