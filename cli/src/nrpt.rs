use anyhow::{bail, Context, Result};
use std::process::Command;

const REMOVE_OURS: &str = "Get-DnsClientNrptRule | \
    Where-Object { $_.Comment -eq 'Hisoka' } | \
    Remove-DnsClientNrptRule -Force";

fn powershell(script: &str) -> Result<String> {
    // Full path, so this works even if PATH is broken.
    let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
    let exe = format!(r"{root}\System32\WindowsPowerShell\v1.0\powershell.exe");
    let out = Command::new(exe)
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", script])
        .output()
        .context("cannot run powershell.exe")?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        bail!("PowerShell failed (administrator terminal needed?):\n{}", err.trim());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Replace all Hisoka NRPT rules with one rule per namespace.
/// Namespaces come from validated patterns (letters, digits, - _ .), so they
/// are safe to place inside single quotes.
pub fn apply(namespaces: &[String]) -> Result<()> {
    let mut script = format!("$ErrorActionPreference = 'Stop'; {REMOVE_OURS}; ");
    for ns in namespaces {
        script.push_str(&format!(
            "Add-DnsClientNrptRule -Namespace '{ns}' -NameServers '127.0.0.1' \
             -Comment 'Hisoka' | Out-Null; "
        ));
    }
    script.push_str("Clear-DnsClientCache");
    powershell(&script).map(|_| ())
}

/// Remove every rule tagged Hisoka. Never touches other NRPT rules.
pub fn unapply() -> Result<()> {
    let script = format!("$ErrorActionPreference = 'Stop'; {REMOVE_OURS}; Clear-DnsClientCache");
    powershell(&script).map(|_| ())
}

pub fn status() -> Result<String> {
    powershell(
        "Get-DnsClientNrptRule | Where-Object { $_.Comment -eq 'Hisoka' } | \
         ForEach-Object { ($_.Namespace -join ',') + ' -> ' + ($_.NameServers -join ',') }",
    )
}