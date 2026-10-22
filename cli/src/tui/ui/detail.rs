//! Drawing the screen. What is shared is in `mod.rs`, each screen its own file.
use super::*;

pub(super) fn draw_detail(frame: &mut Frame, area: Rect, app: &App, modal: bool) {
    // Under an open window the pane is out of focus: the cursor lives in the window.
    let focused = app.focus == Focus::Detail && modal == app.zoom;
    // Named for what is in it — «Transfer», «Account» — rather than
    // «Details» on every screen; the keys are in the bottom line.
    let what = match &app.detail {
        Detail::Operation(d) if d.line.role == "transfer" => trs!("Transfer", "Перевод"),
        Detail::Operation(_) => trs!("Operation", "Операция"),
        Detail::Account(_) => trs!("Account", "Счёт"),
        Detail::Flow(_) => trs!("Flow", "Поток"),
        Detail::Debt(_) => trs!("Debt", "Долг"),
        Detail::Counterparty(_) => trs!("Counterparty", "Контрагент"),
        Detail::Duplicates(_) => trs!("Look-alikes", "Похожие"),
        Detail::Change(_) => trs!("Change", "Правка"),
        Detail::Setting(_) => trs!("Setting", "Настройка"),
        Detail::Category(_) => trs!("Category", "Категория"),
        Detail::Month { .. } => trs!("Month", "Месяц"),
        Detail::Budget { .. } => trs!("Budget", "Бюджет"),
        _ => trs!("Details", "Подробности"),
    };
    let can_focus = !app.detail_items().is_empty();
    let title = match (modal, focused, can_focus) {
        (true, _, _) => tr!("{what} · ↑↓ · Enter — open · x — actions · Esc — close", "{what} · ↑↓ · Enter — открыть · x — действия · Esc — закрыть"),
        (false, true, _) => tr!("{what} · ↑↓ · Enter — open · x — actions · ← back", "{what} · ↑↓ · Enter — открыть · x — действия · ← назад"),
        (false, false, true) => tr!("{what} · →", "{what} · →"),
        (false, false, false) => what.to_string(),
    };
    let block = pane_focused(&title, focused || modal);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let width = inner.width as usize;
    let at = focused.then_some(app.detail_cursor);

    let lines: Vec<Line<'static>> = match &app.detail {
        Detail::Month { month, prev_out, avg_out, flows } => {
            let c = app.month_rule.cycle(month.start, 0);
            let mut out = vec![heading(&format!("{} — {}", c.start.format("%d.%m.%Y"), c.end.format("%d.%m.%Y")))];
            if c.corrected {
                out.push(hint_line(trs!("start corrected by hand · x — change", "начало поправлено руками · x — поменять")));
            }
            let (spent, received) = potok_core::query::flow_totals(flows);
            out.push(kv(trs!("out", "ушло"), bag_text(&spent)));
            out.push(kv(trs!("in", "пришло"), bag_text(&received)));
            if let Some(net_money) = month.net_base {
                let net = net_money.minor();
                let sign = if net > 0 { "+" } else if net < 0 { "−" } else { "" };
                let rate_note = match month.inn_base.map(|m| m.minor()).unwrap_or(0) {
                    inn if inn > 0 && net > 0 => {
                        let pct = (net * 100 + inn / 2) / inn;
                        format!(" ({})", tr!("saved {pct}%", "сохранено {pct}%"))
                    }
                    inn if inn > 0 && net < 0 => {
                        let pct = ((-net) * 100 + inn / 2) / inn;
                        format!(" ({})", tr!("over by {pct}%", "сверх дохода на {pct}%"))
                    }
                    _ => String::new(),
                };
                let text = format!("{sign}{}{rate_note}", potok_core::Money::new(net.abs(), net_money.currency()));
                out.push(Line::from(vec![
                    Span::styled(format!("{:<11}", trs!("net", "итог")), theme::label()),
                    Span::styled(text, theme::money(net)),
                ]));
            }
            let delta_line = |label: &'static str, cur_out: potok_core::Money, ref_out: potok_core::Money| -> Line<'static> {
                let diff = cur_out.minor() - ref_out.minor();
                let sign = if diff > 0 { "+" } else if diff < 0 { "−" } else { "" };
                let pct = if ref_out.minor() > 0 {
                    let p = (diff.abs() * 100 + ref_out.minor() / 2) / ref_out.minor();
                    format!(" ({sign}{p}%)")
                } else {
                    String::new()
                };
                let text = format!("{sign}{}{pct}", potok_core::Money::new(diff.abs(), cur_out.currency()));
                Line::from(vec![
                    Span::styled(format!("{:<11}", label), theme::label()),
                    Span::styled(text, if diff > 0 { theme::spending() } else { theme::ok() }),
                ])
            };
            if let (Some(cur_out), Some(prev)) = (month.out_base, *prev_out) {
                if prev.minor() > 0 || cur_out.minor() > 0 {
                    out.push(delta_line(trs!("vs prev", "к прошлому"), cur_out, prev));
                }
            }
            if let (Some(cur_out), Some(avg)) = (month.out_base, *avg_out) {
                if avg.minor() > 0 {
                    out.push(delta_line(trs!("vs avg", "к среднему"), cur_out, avg));
                }
            }
            let expenses: Vec<&potok_core::query::Flow> = flows.iter().filter(|f| !f.out.is_empty()).collect();
            let incomes: Vec<&potok_core::query::Flow> = flows.iter().filter(|f| !f.inn.is_empty()).collect();
            let label_width = (width / 3).clamp(8, 24);
            let bar_width = width.saturating_sub(label_width + 1 + 15 + 6).max(4);
            let mut item_idx = 0usize;
            if !expenses.is_empty() {
                out.push(Line::raw(""));
                out.push(section(trs!("where it went", "куда ушло")));
                let max = expenses.iter().filter_map(|f| f.out_base.map(|m| m.minor())).max().unwrap_or(0);
                for f in &expenses {
                    let here = at == Some(item_idx);
                    item_idx += 1;
                    let mut line = share_line(f, max, label_width, bar_width);
                    if here {
                        for span in &mut line.spans {
                            span.style = theme::selected();
                        }
                    }
                    out.push(line);
                }
            }
            if !incomes.is_empty() {
                out.push(Line::raw(""));
                out.push(section(trs!("where it came from", "откуда пришло")));
                let max = incomes
                    .iter()
                    .map(|f| first_minor(&f.inn))
                    .max()
                    .unwrap_or(0);
                for f in &incomes {
                    let here = at == Some(item_idx);
                    item_idx += 1;
                    let amount = first_minor(&f.inn);
                    let pick = |base: Style| if here { theme::selected() } else { base };
                    out.push(Line::from(vec![
                        Span::styled(format!("{:<w$} ", fit(&f.label, label_width), w = label_width), pick(theme::value())),
                        Span::styled(bar(amount, max, bar_width), pick(theme::ok())),
                        Span::styled(format!(" {:>14}", bag_text(&f.inn)), pick(theme::money(1))),
                        Span::styled("     ", pick(theme::label())),
                    ]));
                }
            }
            out.push(Line::raw(""));
            out.push(hint_line(if focused {
                trs!("Enter — operations of this row · ← — back to months", "Enter — операции строки · ← — обратно к месяцам")
            } else {
                trs!("Enter — month's operations · → / Tab — pick a row", "Enter — операции месяца · → / Tab — выбрать строку")
            }));
            out
        }
        Detail::Budget { line, trend } => {
            let mut out = vec![heading(&line.category)];
            out.push(kv(trs!("limit", "лимит"), line.limit.to_string()));
            out.push(kv(trs!("spent", "потрачено"), line.spent.to_string()));
            out.push(Line::from(vec![
                Span::styled(format!("{:<11}", trs!("left", "осталось")), theme::label()),
                Span::styled(line.left.to_string(), theme::strong().fg(budget_color(line))),
            ]));
            out.push(Line::raw(""));
            out.push(section(trs!("the last six months", "последние полгода")));
            let max = trend.iter().map(|(_, b)| first_minor(b)).max().unwrap_or(0).max(line.limit.minor());
            let bar_width = width.saturating_sub(8 + 15).max(4);
            for (month, spent) in trend {
                let amount = first_minor(spent);
                let over = amount > line.limit.minor();
                out.push(Line::from(vec![
                    Span::styled(format!("{} ", month.format("%Y-%m")), theme::label()),
                    Span::styled(bar(amount, max, bar_width), if over { theme::error() } else { theme::ok() }),
                    Span::styled(format!(" {:>14}", bag_text(spent)), theme::value()),
                ]));
            }
            out.push(Line::raw(""));
            out.push(hint_line(trs!("Enter — this month's operations · x — change the limit", "Enter — операции месяца · x — поменять лимит")));
            out
        }
        Detail::Category(c) => vec![
            heading(&c.name),
            Line::from(Span::styled(bag_text(&c.spent), theme::strong().fg(theme::bad()))),
            hint_line(&tr!("spent — {}", "потрачено — {}", app.period.label())),
            Line::raw(""),
            kv(trs!("counterparties", "контрагентов"), c.counterparties.to_string()),
            kv(trs!("operations", "операций"), c.operations.to_string()),
            Line::raw(""),
            hint_line(trs!("Enter — all its operations · x — rename, merge, budget, delete", "Enter — все её операции · x — переименовать, свести, бюджет, удалить")),
        ],
        Detail::Setting(key) => {
            let mut out = vec![
                heading(key.label()),
                Line::from(Span::styled(app.setting_value(*key), theme::strong())),
                Line::raw(""),
                Line::from(Span::styled(key.about().to_string(), theme::value())),
                Line::raw(""),
            ];
            if *key == crate::tui::app::SettingKey::Rates {
                for row in &app.rates {
                    let text = match row.rate {
                        Some(rate) => format!("1 {} = {} {}", row.from, potok_core::rate::show(rate), row.to),
                        None => tr!("1 {} = ? {} — no rate", "1 {} = ? {} — нет курса", row.from, row.to),
                    };
                    out.push(Line::from(Span::styled(text, if row.rate.is_some() { theme::strong() } else { theme::attention() })));
                    for (from, to, day, source) in &row.stored {
                        out.push(hint_line(&format!("  {day} {from}→{to} · {source}")));
                    }
                }
                out.push(Line::raw(""));
            }
            out.push(hint_line(trs!("Enter — change", "Enter — поменять")));
            out
        }
        Detail::Nothing => vec![hint_line(trs!("details of the row under the cursor go here", "здесь будут подробности о строке под курсором"))],
        Detail::Operation(d) => {
            let line = &d.line;
            let who = line.counterparty.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| line.description.clone());
            let mut out = vec![
                heading(&who),
                Line::from(Span::styled(line.amount.to_string(), theme::strong().patch(theme::money(line.amount.minor())))),
                Line::raw(""),
                kv(trs!("date", "дата"), line.occurred_on.clone()),
                kv(trs!("account", "счёт"), line.account.clone()),
                kv(trs!("what it is", "что это"), role_name(potok_core::edit::kind(&d.role, line.amount.minor())).to_string()),
            ];
            let category = match (&line.category, &d.category_source) {
                // A transfer does not enter the flows, and its category affects
                // nothing — showing it would pretend that it does.
                _ if d.role != "income" && d.role != "expense" => {
                    tr!("not needed — a {} is not in the flows", "не нужна — {} в потоки не входит", role_name(&d.role))
                }
                (Some(c), Some(potok_core::detail::CategorySource::Own)) => tr!("{c} — its own", "{c} — своя"),
                (Some(c), _) => tr!("{c} — from the counterparty", "{c} — от контрагента"),
                (None, _) => "—".into(),
            };
            out.push(kv(trs!("category", "категория"), category));
            out.push(kv(trs!("note", "заметка"), d.note.clone().unwrap_or_else(|| "—".into())));
            if !d.split.is_empty() {
                out.push(Line::raw(""));
                out.push(section(trs!("split", "разбивка")));
                for (category, amount) in &d.split {
                    out.push(kv(category, amount.to_string()));
                }
            }
            if d.state == "REVERSED" {
                out.push(Line::from(Span::styled(
                    trs!("removed — counts in no balance, flow or debt · x — restore", "убрана — не считается ни в балансе, ни в потоках, ни в долге · x — вернуть"),
                    theme::strong().fg(theme::bad()),
                )));
            }
            out.push(Line::raw(""));
            out.push(section(trs!("description", "описание")));
            out.push(Line::from(Span::styled(line.description.clone(), theme::value())));
            if let Some(pair) = &d.pair {
                out.push(Line::raw(""));
                out.push(section(trs!("the other transfer leg", "вторая нога перевода")));
                out.push(kv(trs!("account", "счёт"), pair.account.clone()));
                out.push(kv(trs!("amount", "сумма"), pair.amount.to_string()));
                out.push(kv(trs!("date", "дата"), pair.occurred_on.clone()));
            }
            if let Some(debt) = &d.debt {
                out.push(Line::raw(""));
                out.push(section(trs!("debt", "долг")));
                out.push(kv(trs!("with", "с кем"), debt.counterparty.clone()));
                out.push(kv(trs!("movement", "движение"), format!("{} · {}", debt.kind, debt.amount)));
            }
            if let Some(src) = &d.source {
                out.push(Line::raw(""));
                out.push(section(trs!("source", "откуда")));
                // A moved row has no original statement row — it has saldo's
                // internal id, and showing that means showing noise.
                if src.bank == "saldo-migration" {
                    out.push(kv("", trs!("migrated from saldo", "перенесено из saldo")));
                } else {
                    out.push(kv(trs!("file", "файл"), format!("{} · {}", src.bank, src.filename)));
                    out.push(kv(trs!("line", "строка"), src.line_no.to_string()));
                    out.push(Line::from(Span::styled(src.raw.clone(), theme::label())));
                }
            }
            // Drilled into a flow row or a period — at the lower right it shows
            // who the row is made of. The core counts it
            // (`group_by_counterparty`), here it is only drawn.
            if !matches!(app.ops_view, OpsView::All) {
                let groups = app.groups();
                if groups.len() > 1 {
                    let shown = app.visible_operations().len();
                    out.push(Line::raw(""));
                    out.push(section(&tr!("who is inside · {} ops", "кто внутри · {} опер.", shown)));
                    // By size rather than by sign: one flow row holds income,
                    // spends and a zero sum — but never zero dealings.
                    let amount = |g: &potok_core::query::Group| g.sum.in_display_order(None).first().map(|m| m.minor().abs()).unwrap_or(0);
                    let mut rows: Vec<&potok_core::query::Group> = groups.iter().collect();
                    rows.sort_by_key(|g| -amount(g));
                    let max = rows.first().map(|g| amount(g)).unwrap_or(0).max(1);
                    let name_width = (width / 3).clamp(8, 20);
                    let bar_width = width.saturating_sub(name_width + 1 + 14 + 6).max(4);
                    for g in rows.iter().take(8) {
                        out.push(Line::from(vec![
                            Span::styled(format!("{:<w$} ", fit(&g.name, name_width), w = name_width), theme::value()),
                            Span::styled(bar(amount(g), max, bar_width), theme::spending()),
                            Span::styled(format!(" {:>13}", bag_text(&g.sum)), theme::value()),
                            Span::styled(format!(" {:>4}", g.count), theme::label()),
                        ]));
                    }
                    if rows.len() > 8 {
                        out.push(hint_line(&tr!("…and {} more", "…и ещё {}", rows.len() - 8)));
                    }
                }
            }
            out
        }
        Detail::Change(c) => {
            let mut out = vec![heading(&tr!("change #{}", "правка #{}", c.id)), kv(trs!("when", "когда"), c.at.clone()), Line::raw("")];
            out.push(Line::from(Span::styled(c.what.clone(), theme::value())));
            out.push(Line::raw(""));
            out.push(match (&c.undone_at, c.can_undo) {
                (Some(at), _) => hint_line(&tr!("undone {at}", "отменена {at}")),
                (None, true) => hint_line(trs!("x — undo: the inverse action, itself journaled", "x — отменить: обратное действие, само попадёт в журнал")),
                (None, false) => hint_line(trs!("cannot be undone — by design", "не отменяется — так устроено")),
            });
            out
        }
        Detail::Duplicates(g) => {
            let first = &g.operations[0];
            let mut out = vec![
                heading(&format!("{} × {}", first.amount, g.operations.len())),
                kv(trs!("account", "счёт"), first.account.clone()),
                kv(trs!("date", "дата"), first.occurred_on.clone()),
                kv(trs!("extra", "лишнего"), tr!("{}, if all but one are copies", "{}, если все кроме одной — копии", g.extra())),
                Line::raw(""),
                section(trs!("copies — Enter open, x remove", "копии — Enter открыть, x убрать")),
            ];
            for (index, op) in g.operations.iter().enumerate() {
                let style = if at == Some(index) { theme::selected() } else { theme::value() };
                out.push(Line::from(Span::styled(
                    fit(&format!("#{} {} · {}", op.id, role_name(potok_core::edit::kind(&op.role, op.amount.minor())), op.description), width),
                    style,
                )));
            }
            out.push(Line::raw(""));
            out.push(hint_line(trs!("two identical fees on one day do happen — then «not a duplicate»", "две одинаковые комиссии в один день бывают — тогда «не дубль»")));
            out
        }
        Detail::Account(d) => {
            let inclusion = match d.inclusion.as_str() {
                "FULL" => trs!("fully in totals", "в итогах полностью"),
                "OFF_BUDGET" => trs!("off budget", "вне бюджета"),
                "EXCLUDED" => trs!("not in totals", "не в итогах"),
                other => other,
            };
            let kind = match d.kind.as_str() {
                "BANK" => trs!("bank account", "счёт в банке"),
                "CARD" => trs!("card", "карта"),
                "CASH" => trs!("cash", "наличные"),
                _ => trs!("other", "другое"),
            };
            let mut out = vec![
                heading(&d.name),
                Line::from(Span::styled(d.balance.to_string(), theme::strong().patch(theme::money(d.balance.minor())))),
                Line::raw(""),
                kv(trs!("opening", "начальный"), d.opening.to_string()),
                kv(trs!("kind", "вид"), format!("{kind} · {inclusion}")),
                kv(trs!("operations", "операций"), format!("{} · {}", d.operations, span_text(&d.first, &d.last))),
                Line::raw(""),
            ];
            out.extend(reconciliation_lines(d.reconciliation.as_ref()));
            if !d.pending.is_empty() {
                out.push(Line::raw(""));
                out.push(section(trs!("pending — not in the balance yet", "ожидают — в балансе их ещё нет")));
                for raw in &d.pending {
                    out.push(Line::from(Span::styled(fit(raw, width), theme::attention())));
                }
            }
            out.push(Line::raw(""));
            out.push(section(trs!("latest", "последние")));
            recent_lines(&mut out, &d.recent, width, at);
            out.push(Line::raw(""));
            out.push(hint_line(trs!("Enter — all operations of the account", "Enter — все операции счёта")));
            out
        }
        Detail::Flow(d) => {
            let mut out = Vec::new();
            if app.screen == Screen::Home {
                if let Some(row) = app.home_flows().get(app.cursor()) {
                    out.push(heading(&row.label));
                    out.push(kv(trs!("out", "ушло"), bag_text(&row.out)));
                    out.push(kv(trs!("in", "пришло"), bag_text(&row.inn)));
                }
            } else if let Some(row) = app.flows.get(app.cursor()) {
                out.push(heading(&row.label));
                out.push(kv(trs!("out", "ушло"), bag_text(&row.out)));
                out.push(kv(trs!("before", "было"), bag_text(&row.out_before)));
                out.push(kv(trs!("in", "пришло"), bag_text(&row.inn)));
            }
            out.push(Line::raw(""));
            out.push(section(trs!("who is inside", "кто внутри")));
            let count_width = tr!(" {:>4} ops", " {:>4} опер", 0).chars().count();
            let name_width = width.saturating_sub(18 + count_width).max(6);
            for (name, bag, count) in d.counterparties.iter().take(10) {
                out.push(Line::from(vec![
                    Span::styled(format!("{:<w$}", fit(name, name_width), w = name_width), theme::value()),
                    Span::styled(format!(" {:>17}", bag_text(bag)), theme::value()),
                    Span::styled(tr!(" {count:>4} ops", " {count:>4} опер"), theme::label()),
                ]));
            }
            if d.counterparties.len() > 10 {
                out.push(hint_line(&tr!("… and {} more", "… и ещё {}", d.counterparties.len() - 10)));
            }
            out.push(Line::raw(""));
            out.push(section(trs!("latest", "последние")));
            recent_lines(&mut out, &d.recent, width, at);
            out.push(Line::raw(""));
            out.push(hint_line(&tr!("Enter — all {} operations of the row", "Enter — все {} операций строки", d.operations)));
            out
        }
        Detail::Debt(d) => {
            let owe = d.direction != "OWED_TO_ME";
            let mut out = vec![
                heading(&d.counterparty),
                Line::from(Span::styled(
                    if d.remaining.minor() == 0 { trs!("settled", "закрыт").to_string() } else if owe { trs!("I owe", "я должен").to_string() } else { trs!("owes me", "мне должен").to_string() },
                    theme::label(),
                )),
                Line::raw(""),
                kv(trs!("left", "осталось"), d.remaining.to_string()),
                kv(trs!("total", "всего"), d.issued.to_string()),
                kv(trs!("opened", "открыт"), d.opened_on.clone()),
                kv(trs!("due", "срок"), d.due_on.clone().unwrap_or_else(|| "—".into())),
                Line::raw(""),
                section(trs!("how it was paid", "как платил")),
            ];
            if d.movements.is_empty() {
                out.push(hint_line(trs!("no movements", "движений нет")));
            }
            let what_width = 14;
            let note_width = width.saturating_sub(11 + what_width + 18 + 2).max(4);
            for (index, m) in d.movements.iter().enumerate() {
                let here = at == Some(index);
                let what = match (m.kind.as_str(), owe) {
                    ("ISSUE", true) => trs!("borrowed", "взял"),
                    ("ISSUE", false) => trs!("lent", "дал"),
                    ("REPAYMENT", true) => trs!("repaid", "вернул"),
                    ("REPAYMENT", false) => trs!("repaid to me", "вернули мне"),
                    ("WRITE_OFF", _) => trs!("written off", "списано"),
                    ("ADJUSTMENT", _) => trs!("correction", "корректировка"),
                    (other, _) => other,
                };
                // No sign: «borrowed 300» and «repaid 100» say the direction,
                // and a signed figure read against the journal's signs —
                // money that came in shown as plus-red. The word carries the
                // colour: the debt grows (warn) or shrinks (good).
                let shown = m.amount.abs();
                let grows = m.kind == "ISSUE";
                let note = if m.counts {
                    m.operation.clone().or_else(|| m.note.clone()).unwrap_or_default()
                } else {
                    trs!("not counted: the operation is removed", "не считается: операция убрана").into()
                };
                let (date_style, what_style, amount_style, note_style) = match (here, m.counts) {
                    (true, _) => (theme::selected(), theme::selected(), theme::selected(), theme::selected()),
                    (false, false) => (theme::label(), theme::label(), theme::label(), theme::label()),
                    (false, true) => {
                        let tone = if grows { theme::attention() } else { theme::ok() };
                        (theme::label(), tone, theme::value(), theme::label())
                    }
                };
                out.push(Line::from(vec![
                    Span::styled(format!("{} ", m.occurred_on), date_style),
                    Span::styled(format!("{what:<what_width$}"), what_style),
                    Span::styled(format!(" {:>17}", shown.to_string()), amount_style),
                    Span::styled(format!("  {}", fit(&note, note_width)), note_style),
                ]));
            }
            out
        }
        Detail::Counterparty(d) => {
            let category = match (&d.category, &d.hint) {
                (Some(c), _) => c.clone(),
                (None, Some((h, n))) => tr!("— ↳ {h}, {n} of {}", "— ↳ {h}, у {n} из {}", d.operations),
                (None, None) => "—".into(),
            };
            let mut out = vec![
                heading(&d.name),
                Line::raw(""),
                kv(trs!("category", "категория"), category),
                kv(trs!("note", "заметка"), d.note.clone().unwrap_or_else(|| "—".into())),
                kv(trs!("operations", "операций"), format!("{} · {}", d.operations, span_text(&d.first, &d.last))),
                kv(trs!("spent", "потрачено"), bag_text(&d.spent)),
                kv(trs!("received", "получено"), bag_text(&d.received)),
                Line::raw(""),
                section(trs!("how banks spell it", "как его пишут банки")),
            ];
            // The spellings are the card's first items: the cursor stands on
            // them, and they are removed from there.
            for (index, alias) in d.aliases.iter().enumerate() {
                let style = if at == Some(index) { theme::selected() } else { theme::label() };
                out.push(Line::from(Span::styled(format!("  {alias}"), style)));
            }
            out.push(Line::raw(""));
            out.push(section(trs!("latest", "последние")));
            recent_lines(&mut out, &d.recent, width, at.map(|at| at.saturating_sub(d.aliases.len())).filter(|_| at.unwrap_or(0) >= d.aliases.len()));
            out.push(Line::raw(""));
            out.push(hint_line(trs!(
                "Enter — all its operations · c — categorise · x — a note, a spelling",
                "Enter — все его операции · c — разметить · x — заметка, написание"
            )));
            out
        }
    };
    let selected_line = focused
        .then(|| {
            lines
                .iter()
                .position(|l| l.spans.iter().any(|s| s.style == theme::selected()))
        })
        .flatten()
        .unwrap_or(0);
    let (scroll_top, _) = window(selected_line, lines.len(), inner.height as usize);
    frame.render_widget(
        Paragraph::new(lines)
            .wrap(Wrap { trim: false })
            .scroll((scroll_top as u16, 0)),
        inner,
    );
}

/// A short list of operations inside the details.
pub(super) fn recent_lines(
    lines: &mut Vec<Line<'static>>,
    ops: &[potok_core::query::OperationLine],
    width: usize,
    selected: Option<usize>,
) {
    let who_width = width.saturating_sub(11 + 18).max(6);
    for (index, op) in ops.iter().enumerate() {
        let who = op.counterparty.as_deref().filter(|n| !n.is_empty()).unwrap_or(&op.description);
        let here = selected == Some(index);
        let (date, name, amount) = if here {
            (theme::selected(), theme::selected(), theme::selected())
        } else {
            (theme::label(), theme::value(), theme::money(op.amount.minor()))
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{} ", op.occurred_on), date),
            Span::styled(format!("{:<w$}", fit(who, who_width), w = who_width), name),
            Span::styled(format!(" {:>17}", op.amount.to_string()), amount),
        ]));
    }
}

pub(super) fn span_text(first: &Option<String>, last: &Option<String>) -> String {
    match (first, last) {
        (Some(first), Some(last)) if first != last => tr!("from {first} to {last}", "с {first} по {last}"),
        (Some(day), _) => day.clone(),
        _ => "—".into(),
    }
}

pub(super) fn role_name(role: &str) -> &str {
    match role {
        "income" => trs!("income", "доход"),
        "expense" => trs!("expense", "расход"),
        "transfer" => trs!("transfer between own accounts", "перевод между своими"),
        "passthrough" => trs!("transit", "транзит"),
        "debt_movement" => trs!("debt movement", "движение долга"),
        "opening_balance" => trs!("opening balance", "начальный остаток"),
        "correction" => trs!("correction", "корректировка"),
        "excluded" => trs!("left out of totals", "исключено из итогов"),
        "refund" => trs!("refund — takes its category's spending down", "возврат — уменьшает траты своей категории"),
        other => other,
    }
}

/// The reconciliation with the bank in an account's details: it agreed, or by
/// how much and what may explain it.
pub(super) fn reconciliation_lines(found: Option<&potok_core::reconcile::Reconciliation>) -> Vec<Line<'static>> {
    let Some(r) = found else {
        return vec![section(trs!("bank reconciliation", "сверка с банком")), hint_line(trs!("never reconciled · s — enter the bank balance", "не сверялся · s — ввести остаток из банка"))];
    };
    let mut out = vec![
        section(trs!("bank reconciliation", "сверка с банком")),
        kv(trs!("on", "на"), r.on_date.clone()),
        kv(trs!("bank says", "в банке"), r.bank.to_string()),
        kv(trs!("here", "здесь"), r.ledger.to_string()),
    ];
    if r.matches() {
        out.push(Line::from(Span::styled(trs!("matches", "сходится"), theme::strong().fg(theme::good()))));
        return out;
    }
    let side = if r.difference.minor() > 0 { trs!("more here", "здесь больше") } else { trs!("less here", "здесь меньше") };
    out.push(Line::from(Span::styled(
        tr!("off by {} — {side}", "расходится на {} — {side}", r.difference.abs()),
        theme::strong().fg(theme::bad()),
    )));
    if r.duplicates_extra.minor() != 0 {
        out.push(kv(trs!("without look-alikes", "без похожих"), tr!("{} · 0 — duplicates", "{} · 0 — дубли", r.without_duplicates())));
    }
    for op in &r.same_amount {
        out.push(kv(trs!("same amount", "та же сумма"), format!("{} {}", op.occurred_on, op.description)));
    }
    out.push(hint_line(trs!("s — reconcile again", "s — сверить снова")));
    out
}
