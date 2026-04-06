use std::path::PathBuf;

const PLIST_LABEL: &str = "com.kittypaw.daemon";
const SYSTEMD_UNIT: &str = "kittypaw.service";

fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            kittypaw_core::secrets::data_dir()
                .unwrap_or_else(|_| PathBuf::from(".kittypaw"))
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."))
                .to_path_buf()
        })
}

fn plist_path() -> PathBuf {
    home_dir()
        .join("Library/LaunchAgents")
        .join(format!("{PLIST_LABEL}.plist"))
}

fn systemd_user_dir() -> PathBuf {
    home_dir().join(".config/systemd/user")
}

fn systemd_unit_path() -> PathBuf {
    systemd_user_dir().join(SYSTEMD_UNIT)
}

fn kittypaw_dir() -> PathBuf {
    kittypaw_core::secrets::data_dir().unwrap_or_else(|_| PathBuf::from(".kittypaw"))
}

pub(crate) fn run_daemon_install() {
    #[cfg(target_os = "linux")]
    {
        install_systemd();
        return;
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        eprintln!("Daemon install is only supported on macOS and Linux.");
        return;
    }

    #[cfg(target_os = "macos")]
    {
        let bin_path = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("kittypaw"));
        let kp_dir = kittypaw_dir();
        std::fs::create_dir_all(&kp_dir).ok();

        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{PLIST_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>serve</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>{}/daemon.log</string>
    <key>StandardErrorPath</key>
    <string>{}/daemon.err</string>
</dict>
</plist>"#,
            bin_path.display(),
            kp_dir.display(),
            kp_dir.display(),
        );

        let path = plist_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        match std::fs::write(&path, &plist) {
            Ok(()) => {
                println!("Plist written to {}", path.display());
                let status = std::process::Command::new("launchctl")
                    .args(["load", "-w"])
                    .arg(&path)
                    .status();
                match status {
                    Ok(s) if s.success() => println!("Daemon installed and started."),
                    Ok(s) => eprintln!("launchctl load exited with: {s}"),
                    Err(e) => eprintln!("Failed to run launchctl: {e}"),
                }
            }
            Err(e) => eprintln!("Failed to write plist: {e}"),
        }
    }
}

#[cfg(target_os = "linux")]
fn install_systemd() {
    let bin_path = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("kittypaw"));
    let kp_dir = kittypaw_dir();
    std::fs::create_dir_all(&kp_dir).ok();

    let unit = format!(
        r#"[Unit]
Description=KittyPaw AI Automation Agent
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
ExecStart={} serve
Restart=on-failure
RestartSec=5
WorkingDirectory={}
Environment=KITTYPAW_LOG_FORMAT=json

[Install]
WantedBy=default.target
"#,
        bin_path.display(),
        kp_dir.display(),
    );

    let dir = systemd_user_dir();
    std::fs::create_dir_all(&dir).ok();
    let path = systemd_unit_path();

    match std::fs::write(&path, &unit) {
        Ok(()) => {
            println!("Unit file written to {}", path.display());
            let _ = std::process::Command::new("systemctl")
                .args(["--user", "daemon-reload"])
                .status();
            let status = std::process::Command::new("systemctl")
                .args(["--user", "enable", "--now", SYSTEMD_UNIT])
                .status();
            match status {
                Ok(s) if s.success() => println!("Daemon installed and started."),
                Ok(s) => eprintln!("systemctl enable exited with: {s}"),
                Err(e) => eprintln!("Failed to run systemctl: {e}"),
            }
        }
        Err(e) => eprintln!("Failed to write unit file: {e}"),
    }
}

pub(crate) fn run_daemon_uninstall() {
    #[cfg(target_os = "linux")]
    {
        let path = systemd_unit_path();
        if path.exists() {
            match std::process::Command::new("systemctl")
                .args(["--user", "disable", "--now", SYSTEMD_UNIT])
                .status()
            {
                Ok(s) if !s.success() => {
                    eprintln!("Warning: systemctl disable exited with: {s}");
                }
                Err(e) => eprintln!("Warning: failed to run systemctl: {e}"),
                _ => {}
            }
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    let _ = std::process::Command::new("systemctl")
                        .args(["--user", "daemon-reload"])
                        .status();
                    println!("Daemon uninstalled.");
                }
                Err(e) => eprintln!("Failed to remove unit file: {e}"),
            }
        } else {
            println!("Daemon is not installed.");
        }
        return;
    }

    #[cfg(target_os = "macos")]
    {
        let path = plist_path();
        if path.exists() {
            let _ = std::process::Command::new("launchctl")
                .args(["unload"])
                .arg(&path)
                .status();
            match std::fs::remove_file(&path) {
                Ok(()) => println!("Daemon uninstalled."),
                Err(e) => eprintln!("Failed to remove plist: {e}"),
            }
        } else {
            println!("Daemon is not installed.");
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    eprintln!("Daemon is not supported on this platform.");
}

pub(crate) fn run_daemon_status() {
    #[cfg(target_os = "linux")]
    {
        let path = systemd_unit_path();
        if !path.exists() {
            println!("Daemon: not installed");
            return;
        }
        println!("Unit: {}", path.display());
        let output = std::process::Command::new("systemctl")
            .args(["--user", "is-active", SYSTEMD_UNIT])
            .output();
        match output {
            Ok(out) => {
                let status = String::from_utf8_lossy(&out.stdout).trim().to_string();
                println!("Status: {status}");
            }
            Err(_) => println!("Status: unknown (systemctl failed)"),
        }
        return;
    }

    #[cfg(target_os = "macos")]
    {
        let path = plist_path();
        if !path.exists() {
            println!("Daemon: not installed");
            return;
        }
        println!("Plist: {}", path.display());
        let output = std::process::Command::new("launchctl")
            .args(["list"])
            .output();
        match output {
            Ok(out) => {
                let stdout = String::from_utf8_lossy(&out.stdout);
                if stdout.contains(PLIST_LABEL) {
                    println!("Status: running");
                } else {
                    println!("Status: installed but not running");
                }
            }
            Err(_) => println!("Status: unknown (launchctl failed)"),
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    println!("Daemon: not supported on this platform");
}
