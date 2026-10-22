//! The settings screen: what it shows and what it changes.
use super::*;

impl App {
    /// Settings from the ledger: language for this thread, and the values
    /// screens read.
    pub fn load_settings(&mut self) -> Result<()> {
        use potok_core::settings;
        settings::apply_language(&self.conn)?;
        let theme = settings::get(&self.conn, settings::THEME)?.as_deref().and_then(crate::tui::theme::Theme::from_code);
        crate::tui::theme::set(theme.unwrap_or(crate::tui::theme::Theme::Terminal));
        self.statements_dir = settings::get(&self.conn, settings::STATEMENTS_DIR)?;
        self.default_account = settings::get(&self.conn, settings::DEFAULT_ACCOUNT)?.and_then(|v| v.parse().ok());
        self.month_rule = potok_core::period::MonthRule::load(&self.conn)?;
        if let Some(period) = settings::get(&self.conn, settings::FLOWS_PERIOD)?.as_deref().and_then(Period::from_code) {
            self.period = period;
        }
        Ok(())
    }

    /// What a settings row says now.
    pub fn setting_value(&self, key: SettingKey) -> String {
        match key {
            SettingKey::Language => potok_core::i18n::lang().name().to_string(),
            SettingKey::Theme => crate::tui::theme::current().code().to_string(),
            SettingKey::StatementsDir => self.statements_folder(),
            SettingKey::DefaultAccount => match self.default_account.and_then(|id| self.accounts.iter().find(|a| a.id == id)) {
                Some(account) => account.name.clone(),
                None => tr!("Auto", "Авто"),
            },
            SettingKey::FlowsPeriod => self.period_on_start().label().to_string(),
            SettingKey::TransferDays => format!("±{}", potok_core::settings::transfer_days(&self.conn).unwrap_or(3)),
            SettingKey::BaseCurrency => potok_core::settings::base_currency(&self.conn).map(|c| c.to_string()).unwrap_or_default(),
            SettingKey::Rates => {
                let missing = self.rates.iter().filter(|r| r.rate.is_none()).count();
                let auto = potok_core::settings::get(&self.conn, potok_core::settings::RATES_AUTO).ok().flatten().as_deref() == Some("on");
                let fetched = potok_core::settings::get(&self.conn, potok_core::settings::RATES_FETCHED).ok().flatten();
                let mut text = match (self.rates.len(), missing) {
                    (0, _) => tr!("one currency — none needed", "одна валюта — не нужны"),
                    (_, 0) => tr!("all there", "все есть"),
                    (_, n) => tr!("{n} missing — Enter", "нет {n} — Enter"),
                };
                if let Some(day) = fetched {
                    text.push_str(&tr!(" · fetched {day}", " · загружены {day}"));
                }
                if auto {
                    text.push_str(trs!(" · daily", " · каждый день"));
                }
                text
            }
            SettingKey::BackupDir => self.backup_dir().map(|d| d.display().to_string()).unwrap_or_default(),
            SettingKey::ScreenLock => if self.lock_hash().is_some() { tr!("on — L locks", "включена — L блокирует") } else { tr!("off", "выключена") },
            SettingKey::MonthDay => match self.month_rule.kind {
                potok_core::period::Kind::LastWorkday => tr!("on the last working day", "с последнего рабочего дня"),
                potok_core::period::Kind::Day(1) => tr!("on the 1st — calendar months", "с 1-го — календарные месяцы"),
                potok_core::period::Kind::Day(day) => tr!("on day {day}", "с {day}-го числа"),
            },
            SettingKey::LockAfter => match self.lock_after() {
                Some(minutes) => tr!("after {minutes} min", "через {minutes} мин"),
                None if self.lock_hash().is_none() => tr!("needs a passphrase", "нужен пароль"),
                None => tr!("never", "никогда"),
            },
            SettingKey::MonthStart => {
                let c = self.month_rule.cycle(self.today, 0);
                let how = if c.corrected { tr!("corrected by hand", "поправлено руками") } else { tr!("by the rule", "по правилу") };
                format!("{} — {} · {how}", c.start.format("%d.%m.%Y"), c.end.format("%d.%m.%Y"))
            }
            SettingKey::Device => {
                let name = crate::cloud::device_name(&self.conn);
                match &self.read_only {
                    Some(mark) => tr!("{name} · read only, {} writes", "{name} · только чтение, пишет {}", mark.device),
                    None => name,
                }
            }
            SettingKey::Cloud => match (&self.cloud_error, potok_core::settings::get(&self.conn, potok_core::settings::CLOUD_SENT).ok().flatten()) {
                (Some(error), _) => tr!("failed: {}", "не ушла: {}", error),
                (None, Some(sent)) => tr!("sent {sent}", "ушла {sent}"),
                (None, None) => tr!("off — potok cloud login", "выключена — potok cloud login"),
            },
            SettingKey::BackupNow => match self
                .backup_dir()
                .ok()
                .and_then(|d| potok_core::backup::list(&d).into_iter().next())
            {
                Some((at, _)) => chrono::DateTime::<chrono::Local>::from(at).format("%Y-%m-%d %H:%M").to_string(),
                None => tr!("none yet", "ещё не было"),
            },
        }
    }

    pub(super) fn period_on_start(&self) -> Period {
        potok_core::settings::get(&self.conn, potok_core::settings::FLOWS_PERIOD)
            .ok()
            .flatten()
            .as_deref()
            .and_then(Period::from_code)
            .unwrap_or(Period::ThisMonth)
    }

    /// Enter on a settings row: the next choice, or a field to type in.
    pub fn change_setting(&mut self) -> Result<()> {
        use potok_core::settings;
        let Some(key) = SettingKey::ALL.get(self.cursor()).copied() else { return Ok(()) };
        match key {
            SettingKey::Language => {
                let next = match potok_core::i18n::lang() {
                    potok_core::i18n::Lang::En => potok_core::i18n::Lang::Ru,
                    potok_core::i18n::Lang::Ru => potok_core::i18n::Lang::En,
                };
                settings::set(&self.conn, settings::LANGUAGE, next.code())?;
            }
            SettingKey::Theme => {
                let themes = crate::tui::theme::Theme::ALL;
                let at = themes.iter().position(|t| *t == crate::tui::theme::current()).unwrap_or(0);
                settings::set(&self.conn, settings::THEME, themes[(at + 1) % themes.len()].code())?;
            }
            SettingKey::StatementsDir => return self.run(MenuAction::Ask(Ask::StatementsDir)),
            SettingKey::BackupDir => return self.run(MenuAction::Ask(Ask::BackupDir)),
            SettingKey::ScreenLock => {
                let ask = if self.lock_hash().is_some() { Ask::LockOff } else { Ask::LockSet };
                return self.run(MenuAction::Ask(ask));
            }
            SettingKey::MonthDay => {
                // 1, 5, 10, 15, 20, 25, the last working day, and round again.
                match self.month_rule.kind {
                    potok_core::period::Kind::LastWorkday => {
                        settings::set(&self.conn, settings::MONTH_MODE, "day")?;
                        settings::set(&self.conn, settings::MONTH_DAY, "1")?;
                    }
                    potok_core::period::Kind::Day(day) => match [1, 5, 10, 15, 20, 25].into_iter().find(|d| *d > day) {
                        Some(next) => {
                            settings::set(&self.conn, settings::MONTH_MODE, "day")?;
                            settings::set(&self.conn, settings::MONTH_DAY, &next.to_string())?;
                        }
                        None => settings::set(&self.conn, settings::MONTH_MODE, "last_workday")?,
                    },
                }
            }
            SettingKey::LockAfter => {
                if self.lock_hash().is_none() {
                    self.status = Some(tr!("set a passphrase first — Screen lock", "сначала пароль — Блокировка экрана"));
                    return Ok(());
                }
                let choices = [1u64, 5, 10, 15, 30];
                match self.lock_after().and_then(|now| choices.iter().position(|c| *c == now)) {
                    Some(at) if at + 1 < choices.len() => settings::set(&self.conn, settings::LOCK_AFTER, &choices[at + 1].to_string())?,
                    Some(_) => settings::clear(&self.conn, settings::LOCK_AFTER)?,
                    None => settings::set(&self.conn, settings::LOCK_AFTER, "1")?,
                }
            }
            SettingKey::MonthStart => {
                let key = self.month_rule.cycle(self.today, 0).key;
                return self.run(MenuAction::Ask(Ask::MonthStart(key)));
            }
            SettingKey::BaseCurrency => {
                let mut codes: Vec<String> = self.accounts.iter().map(|a| a.balance.currency().to_string()).collect();
                codes.push("EUR".into());
                let mut seen = std::collections::HashSet::new();
                codes.retain(|c| seen.insert(c.clone()));
                let now = settings::base_currency(&self.conn)?.to_string();
                let at = codes.iter().position(|c| *c == now).unwrap_or(0);
                settings::set(&self.conn, settings::BASE_CURRENCY, &codes[(at + 1) % codes.len()])?;
            }
            SettingKey::Rates => return self.run(MenuAction::RatesMenu),
            SettingKey::TransferDays => {
                let choices = settings::TRANSFER_DAYS_CHOICES;
                let now = settings::transfer_days(&self.conn)?;
                let at = choices.iter().position(|d| *d == now).unwrap_or(0);
                settings::set(&self.conn, settings::TRANSFER_DAYS, &choices[(at + 1) % choices.len()].to_string())?;
            }
            SettingKey::Device => return self.run(MenuAction::Ask(Ask::DeviceName)),
            SettingKey::Cloud => {
                // Signing in needs a browser and a typed passphrase — a
                // terminal's job, not this screen's.
                self.status = Some(trs!("in a terminal: potok cloud login · potok cloud push", "в терминале: potok cloud login · potok cloud push").into());
                return Ok(());
            }
            SettingKey::BackupNow => {
                let made = potok_core::backup::make(&self.conn, &self.backup_dir()?)?;
                self.status = Some(tr!("backup: {}", "бэкап: {}", made.display()));
                return Ok(());
            }
            SettingKey::DefaultAccount => {
                // Auto, then each account in turn, then Auto again.
                let ids: Vec<i64> = self.accounts.iter().map(|a| a.id).collect();
                let at = self.default_account.and_then(|id| ids.iter().position(|i| *i == id));
                match at.map(|i| i + 1).unwrap_or(0) {
                    i if i < ids.len() => settings::set(&self.conn, settings::DEFAULT_ACCOUNT, &ids[i].to_string())?,
                    _ => settings::clear(&self.conn, settings::DEFAULT_ACCOUNT)?,
                }
            }
            SettingKey::FlowsPeriod => {
                let at = Period::ALL.iter().position(|p| *p == self.period_on_start()).unwrap_or(0);
                let next = Period::ALL[(at + 1) % Period::ALL.len()];
                settings::set(&self.conn, settings::FLOWS_PERIOD, next.code())?;
                self.period = next;
            }
        }
        self.load_settings()?;
        self.reload()
    }

    /// The folder `i` looks in, as shown and as typed: the setting, or
    /// `~/Downloads`.
    pub fn statements_folder(&self) -> String {
        self.statements_dir.clone().unwrap_or_else(|| "~/Downloads".into())
    }

    /// Where backups go; an in-memory ledger (tests) has none.
    pub(super) fn backup_dir(&self) -> Result<std::path::PathBuf> {
        let db = self.conn.path().filter(|p| !p.is_empty()).map(std::path::PathBuf::from);
        let db = db.unwrap_or_else(|| std::env::temp_dir().join("potok-memory").join("ledger.db"));
        potok_core::backup::dir(&self.conn, &db)
    }

}
