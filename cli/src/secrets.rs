//! Where the encryption passphrase and the Google sign-in are kept.
//!
//! On macOS — in the Keychain through `security`: logging in unlocks it, and no
//! separate passphrase is needed. Other systems have no Keychain, and the
//! secret lies in `~/.config/potok/secrets` with mode 0600 — the same as `gh`,
//! `aws` and `docker` do. The difference matters and is named in the README:
//! the file is readable by anyone who has already become this user, and by
//! anyone who got a copy of the home folder.
//!
//! The values are stored as a hexadecimal string: that way quotes, spaces and
//! Cyrillic in a passphrase survive both `security` and parsing the file by lines.
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use potok_core::tr;

const SERVICE: &str = "potok";

/// Only a Mac has a Keychain. Checked by the platform rather than by `security`
/// being on PATH: a substituted `security` in someone else's PATH must not
/// decide where the passphrase lives.
fn keychain() -> bool {
    cfg!(target_os = "macos")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    let text = text.trim();
    if text.len() % 2 != 0 {
        return None;
    }
    (0..text.len()).step_by(2).map(|i| u8::from_str_radix(text.get(i..i + 2)?, 16).ok()).collect()
}

/// The secrets file: `$XDG_CONFIG_HOME/potok/secrets`, else
/// `~/.config/potok/secrets`. Beside the settings rather than the ledger: the
/// ledger travels to the cloud, and the cloud's passphrase must not travel with it.
pub fn file() -> Result<PathBuf> {
    let base = match std::env::var_os("XDG_CONFIG_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(std::env::var_os("HOME").context(tr!("no HOME", "нет HOME"))?).join(".config"),
    };
    Ok(base.join("potok").join("secrets"))
}

fn read_file() -> Result<Vec<(String, String)>> {
    let path = file()?;
    let Ok(text) = std::fs::read_to_string(&path) else { return Ok(Vec::new()) };
    Ok(text
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .map(|(account, value)| (account.to_string(), value.to_string()))
        .collect())
}

fn write_file(entries: &[(String, String)]) -> Result<()> {
    let path = file()?;
    let dir = path.parent().context("secrets dir")?;
    std::fs::create_dir_all(dir)?;
    potok_core::db::owner_only(dir)?;
    let text: String = entries.iter().map(|(a, v)| format!("{a}\t{v}\n")).collect();
    potok_core::db::write_private(&path, text.as_bytes())
}

/// The secret, or `None` if there is none.
pub fn get(account: &str) -> Result<Option<String>> {
    let stored = if keychain() {
        let out = Command::new("security")
            .args(["find-generic-password", "-s", SERVICE, "-a", account, "-w"])
            .output()
            .context("security")?;
        if !out.status.success() {
            return Ok(None);
        }
        String::from_utf8_lossy(&out.stdout).to_string()
    } else {
        match read_file()?.into_iter().find(|(name, _)| name == account) {
            Some((_, value)) => value,
            None => return Ok(None),
        }
    };
    let bytes = unhex(&stored).context(tr!("a stored secret is not potok's", "сохранённый секрет не от potok"))?;
    Ok(Some(String::from_utf8(bytes).context("utf-8")?))
}

/// Saves a secret. On a Mac — through `security -i` on stdin: on the command
/// line any process could read it while `security` runs.
pub fn put(account: &str, value: &str) -> Result<()> {
    let value = hex(value.as_bytes());
    if !keychain() {
        let mut entries = read_file()?;
        entries.retain(|(name, _)| name != account);
        entries.push((account.to_string(), value));
        return write_file(&entries);
    }
    let mut child = Command::new("security").arg("-i").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn().context("security")?;
    let line = format!("add-generic-password -U -s {SERVICE} -a {account} -w {value}\n");
    child.stdin.take().context("stdin")?.write_all(line.as_bytes())?;
    let out = child.wait_with_output()?;
    let errors = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() || errors.contains("rror") {
        bail!(tr!("the Keychain refused: {}", "Keychain отказал: {}", errors.trim()));
    }
    Ok(())
}

pub fn forget(account: &str) {
    if keychain() {
        let _ = Command::new("security").args(["delete-generic-password", "-s", SERVICE, "-a", account]).output();
        return;
    }
    if let Ok(mut entries) = read_file() {
        entries.retain(|(name, _)| name != account);
        let _ = write_file(&entries);
    }
}

/// Where they live — in one line, for the README and for `potok cloud`.
pub fn where_they_live() -> String {
    if keychain() {
        tr!("the login Keychain", "связка ключей входа")
    } else {
        file().map(|p| p.display().to_string()).unwrap_or_else(|_| "~/.config/potok/secrets".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The file store survives quotes, spaces and Cyrillic and does not leave
    /// the file readable by neighbours on the machine.
    #[test]
    fn a_secret_in_a_file_comes_back_as_it_went_in() {
        let dir = std::env::temp_dir().join(format!("potok-secrets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("XDG_CONFIG_HOME", &dir);
        assert!(file().unwrap().starts_with(&dir));

        let secret = "пароль \"с\" кавычками и пробелами";
        let entries = vec![("passphrase".to_string(), hex(secret.as_bytes()))];
        write_file(&entries).unwrap();
        let back = read_file().unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(String::from_utf8(unhex(&back[0].1).unwrap()).unwrap(), secret);

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(file().unwrap()).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "only the owner can read the secret");
        }
        std::env::remove_var("XDG_CONFIG_HOME");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
