//! Loading a statement and parsing it before writing.
use super::*;

impl App {
    /// `i` on the import screen: the statements from «Downloads», the new on top, and «type a path».
    pub fn start_import(&mut self) {
        self.menu = recent_statements(&self.statements_folder())
            .into_iter()
            .map(|path| MenuItem {
                label: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
                action: MenuAction::ImportFile(path),
                confirm: None,
            })
            .collect();
        self.menu.push(MenuItem { label: trs!("Enter a path…", "Ввести путь…").into(), action: MenuAction::ImportPath, confirm: None });
        self.mode = Mode::Menu { selected: 0 };
    }

    /// Enter while typing a path.
    pub fn apply_path(&mut self) -> Result<()> {
        self.mode = Mode::Normal;
        let typed = self.input.trim();
        let path = match (typed.strip_prefix("~/"), home_dir()) {
            (Some(rest), Some(home)) => home.join(rest),
            _ => std::path::PathBuf::from(typed),
        };
        self.run_reporting(MenuAction::ImportFile(path))
    }

    pub(super) fn start_review(
        &mut self,
        path: std::path::PathBuf,
        statement: potok_core::import::Statement,
        account: &str,
    ) -> Result<()> {
        let plan = potok_core::import::review::plan(&self.conn, account, &path, &statement, self.today)?;
        if plan.already_imported {
            self.status = Some(trs!("this file is already imported — nothing to write", "этот файл уже загружен — записывать нечего").into());
            return Ok(());
        }
        self.review = Some(Review { path, statement, plan, cursor: 0 });
        self.status = None;
        self.mode = Mode::Review;
        Ok(())
    }

    pub fn move_review(&mut self, by: isize) {
        if let Some(review) = &mut self.review {
            let count = review.new_rows().len();
            if count > 0 {
                review.cursor = (review.cursor as isize + by).clamp(0, count as isize - 1) as usize;
            }
        }
    }

    /// Space: write the row under the cursor or not.
    pub fn toggle_review_write(&mut self) {
        if let Some(review) = &mut self.review {
            if let Some(&at) = review.new_rows().get(review.cursor) {
                review.plan.rows[at].write = !review.plan.rows[at].write;
            }
        }
    }

    /// Enter: accept the hint under the cursor or refuse it.
    pub fn toggle_review_hint(&mut self) {
        if let Some(review) = &mut self.review {
            if let Some(&at) = review.new_rows().get(review.cursor) {
                let row = &mut review.plan.rows[at];
                if row.hint.is_some() {
                    row.accept_hint = !row.accept_hint;
                }
            }
        }
    }

    /// `w`: write it — after a «yes».
    pub fn ask_write_import(&mut self) {
        let Some(review) = &self.review else { return };
        let plan = &review.plan;
        self.pending = Some(MenuItem {
            label: String::new(),
            action: MenuAction::WriteImport,
            confirm: Some(tr!(
                "Write {} operations to {}? Transfers {}, debt payments {}", "Записать {} операций на {}? Переводов {}, по долгам {}",
                plan.to_write(),
                plan.account,
                plan.accepted(true),
                plan.accepted(false)
            )),
        });
        self.mode = Mode::Confirm;
    }

    /// Esc in the review: what was decided there — rows left out, hints
    /// turned down — goes with it, so ask first.
    pub fn ask_drop_review(&mut self) {
        self.pending = Some(MenuItem {
            label: String::new(),
            action: MenuAction::DropReview,
            confirm: Some(trs!(
                "Drop this statement? Nothing is written, and the choices made here are lost",
                "Бросить эту выписку? Ничего не запишется, решения в разборе пропадут"
            ).into()),
        });
        self.mode = Mode::Confirm;
    }

    pub fn cancel_review(&mut self) {
        self.review = None;
        self.mode = Mode::Normal;
        self.status = Some(trs!("statement not written", "выписка не записана").into());
    }

    /// The archive of originals sits beside the database; for an in-memory one
    /// (the tests) — in a temporary folder.
    pub(super) fn archive_dir(&self) -> std::path::PathBuf {
        match self.conn.path().filter(|p| !p.is_empty()) {
            Some(path) => potok_core::db::archive_dir(std::path::Path::new(path)),
            None => std::env::temp_dir().join("potok-archive"),
        }
    }

}
