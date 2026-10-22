//! The screen lock.
use super::*;

impl App {
    pub fn lock_hash(&self) -> Option<String> {
        potok_core::settings::get(&self.conn, potok_core::settings::LOCK_HASH).ok().flatten()
    }

    /// Lock now, if a passphrase is set.
    pub fn lock(&mut self) {
        if self.lock_hash().is_some() {
            self.input.clear();
            self.status = None;
            self.asking = None;
            self.opening_for = None;
            self.pending = None;
            self.form = None;
            self.review = None;
            self.queue = false;
            self.mode = Mode::Locked;
            self.locked_at = Some(chrono::Local::now());
        } else {
            self.status = Some(tr!("no lock set — , → Screen lock", "блокировка не задана — , → Блокировка экрана"));
        }
    }

    /// Seconds still to wait after wrong passphrases.
    pub fn lock_wait(&self) -> u64 {
        self.lock_wait_until
            .and_then(|until| until.checked_duration_since(std::time::Instant::now()))
            .map(|left| left.as_secs() + 1)
            .unwrap_or(0)
    }

    /// Enter on the lock: open on the right passphrase. A wrong one makes the
    /// next try wait — one second, then two, four, up to half a minute — so
    /// guessing at the keyboard is slow, while a typo costs a moment.
    pub fn unlock(&mut self) {
        if self.lock_wait() > 0 {
            return;
        }
        let ok = self.lock_hash().is_none_or(|h| crate::tui::lock::verify(&self.input, &h));
        self.input.clear();
        if ok {
            self.mode = Mode::Normal;
            self.status = None;
            self.lock_failures = 0;
            self.lock_wait_until = None;
            self.locked_at = None;
        } else {
            self.lock_failures += 1;
            let wait = (1u64 << (self.lock_failures - 1).min(5)).min(30);
            self.lock_wait_until = Some(std::time::Instant::now() + std::time::Duration::from_secs(wait));
            self.status = Some(tr!("wrong passphrase — attempt {}", "пароль не тот — попытка {}", self.lock_failures));
        }
    }

    /// Minutes of quiet after which the screen locks; `None` — never.
    pub fn lock_after(&self) -> Option<u64> {
        self.lock_hash()?;
        potok_core::settings::get(&self.conn, potok_core::settings::LOCK_AFTER).ok().flatten()?.parse().ok()
    }

}
