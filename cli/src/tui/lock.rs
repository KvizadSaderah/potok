//! The passphrase that unlocks the interface.
//!
//! Ported from saldo's client, and with its warning: this is a **screen
//! lock** — it stops somebody at an unattended terminal from reading a ledger
//! that is open. It is **not** protection for the file: the ledger is plain
//! SQLite, and whoever can read it does not need this program. The settings
//! row says so.
//!
//! Only an Argon2id hash with a fresh salt is kept, in the ledger's settings —
//! never the passphrase. It is not the password of anything else; reusing one
//! would put a hash of a real credential into the ledger and its backups.
use anyhow::{bail, Result};
use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use potok_core::tr;

/// Below this a passphrase is not one.
pub const MINIMUM: usize = 6;

pub fn hash(passphrase: &str) -> Result<String> {
    if passphrase.chars().count() < MINIMUM {
        bail!(tr!("a passphrase needs at least {MINIMUM} characters", "в пароле нужно хотя бы {MINIMUM} символов"));
    }
    let salt = SaltString::generate(&mut OsRng);
    Ok(Argon2::default()
        .hash_password(passphrase.as_bytes(), &salt)
        .map_err(|error| anyhow::anyhow!("{error}"))?
        .to_string())
}

/// A stored string that cannot be parsed says no: it is a corrupted lock, not
/// an open door.
pub fn verify(passphrase: &str, stored: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored) else { return false };
    Argon2::default().verify_password(passphrase.as_bytes(), &parsed).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_right_passphrase_opens_and_short_ones_are_refused() {
        let stored = hash("correct horse").unwrap();
        assert!(!stored.contains("correct"));
        assert!(verify("correct horse", &stored));
        assert!(!verify("wrong horse", &stored));
        assert!(!verify("correct horse", "not a hash"));
        assert!(hash("short").is_err());
    }
}
