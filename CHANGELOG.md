# What is new in potok

`potok --version` says which one is installed and from which commit (`+` — it
was built with uncommitted changes). `potok update --check` says whether the
source has moved on since.

## 0.2.0 — 2026-10-20

**Google Drive is out of the way.** Starting no longer waits for Drive (two or
three seconds, longer with no network): the ledger opens at once and the check
runs beside it — another device's mark turns the screen read-only where it
stands. Leaving sends the copy in a process of its own, so quitting is
quitting. `potok cloud push` still sends on demand.

**A name from Drive cannot leave the backup folder.** It is used as a file
name here and as a value in curl's config, and both had a way out — a path
escape and an extra curl option. One check in each place now, with a test that
fails without them.

**Money.** A refund is taken off the shop's spending instead of counting as
income (`potok refunds apply` for rows written earlier). «Where it went»,
the order of flows and categories are in the base currency, so a rouble row
no longer outranks a euro one by the size of its number.

**The ledger.** Schema 7: an operation's number is never reused, so a journal
entry cannot undo somebody else's edit after an import rollback. Opening a
ledger of your own version writes nothing — the «database is locked» refusals
are gone. `restore` and `backup` no longer take a backup of the broken state
first.

**The interface.** Read-only offers no edits and no longer quits on a key that
writes; a failed key is a line at the bottom. Help shows this screen's keys
first. Tabs keep their names at 80 columns. The «terminal» theme shows the
row under the cursor.

**Statements.** Both parsers were checked against a hundred synthetic
statements each with a known answer, and against an independent parse of the
two real ones: a number at the end of a Revolut description no longer loses
the row, a pending top-up is no longer written as money, an unknown iCard
status no longer swallows the next record, and a line in another currency is
refused before anything is written instead of failing mid-write.

**Home is lists, not a picture.** The cursor walks «where it went» and
`Enter` opens the operations behind a row — what «Other» is made of is two
keys away. `Tab` moves on to the months along the bottom of the chart: the
column under the cursor says what that month cost and brought, `Enter` opens
its operations (the row menu also corrects when that month starts), and
«where it went» follows the cursor — a month that is over is shown with its
name and its dates on the block, never under this month's heading.
`Tab` again crosses to the right side, where every line is the thing
itself: a regular payment opens that counterparty's operations, a debt opens
on the Debts screen, and a line in «Needs a look» does what it says — starts
the reconciliation of that account, walks the counterparties without a
category, asks whether to count the refunds as refunds. `Esc` comes back.

**One correction moves the shop.** Give an operation its own category and
potok asks whether the rest of that counterparty's operations should move
there too and be remembered. «Yes» makes it the counterparty's category and
takes its operations out of the category they sat in — a migrated history
where the category was written on every row follows the rule from then on.

**A note of your own on a counterparty.** The bank writes «NORD OOD SOFIA BGR»
and only the person knows what the place is. Schema 8 keeps a note beside the
name, the counterparty's details show it, `potok counterparty note <who>
<text>` writes it and without text shows it; it is a line in the journal and
the undo puts the previous one back.

**Counterparties have a screen of their own.** The directory used to live on
the Import screen, where nobody would look for it: `8` now opens
«Counterparties» — everyone who is paid or pays, what they cost, their
category and the hint when they have none. What was loaded moved to
«Statements» (`I`, since the digits ran out at ten screens), where `Enter`
shows what a statement brought and `x` rolls one back.

**The whole card is editable.** The counterparty's spellings — the keys a
statement is matched by — used to be shown and nothing more: `x` on the card
adds one, and the cursor standing on a spelling removes it. `potok alias
remove <text>` does the same without the interface, and tying a spelling is
now a line in the journal like every other edit.

**Themes, and the screen moving.** «saldo» and «light» are gone — two
palettes nobody kept on. What is left is **terminal** (the sixteen colours
the terminal is set to, which also covers a light background), and three new
ones: **midnight**, **amber** and **mono**. A ledger that was on a removed
theme opens on «terminal». Bars and lists now grow into place over a third of
a second when a screen opens, at sixty frames a second while they do and the
old four a second once they are still.

**A note of your own on an operation.** Schema 9 keeps a line the bank did
not write beside the row — «give it back to Kolya», «this was the repair». It
shows on the operation's card, `x` → «A note of your own…» writes it, `potok
op <n> note <text>` does it without the interface, and the undo puts the
previous one back. The amount, the date and the account of a posted operation
still never change (invariant 8) — a note is not money.

**Six themes, and the cursor keeps the row's colours.** The row under the
cursor used to be drawn in reverse on the «terminal» theme — black on white,
the one place where colour meant something else. Now every theme puts it on
its own background and leaves the amounts red and green. Two more palettes:
**neon** (ultraviolet, pink structure, turquoise income) and **green** (a
phosphor screen, money still red and green). Switching a screen lights its
tab up for a moment before it settles.

**The cloud works beyond macOS.** The passphrase and the Google sign-in used
to live in the Keychain and nowhere else, so a Linux machine could keep a
ledger but never share it. They now live in `~/.config/potok/secrets` (0600)
where there is no Keychain, the browser opens with `xdg-open`, and the device
is named after its hostname. `potok cloud` says where the secrets are.

**Commands.** `potok op`, `category`, `month`, `debt due|note|delete`,
`account opening`, `duplicates keep`, `refunds`, `role … refund`, and
`version` / `update`. `potok flows` without dates uses the accounting month.

## 0.1.0 — 2026-10-16 … 19

The first one that holds a real ledger: 1774 operations migrated from saldo
and reconciled to the cent. Import of Revolut and iCard PDFs with a review
before anything is written; accounts, operations, flows, analytics, budget,
debts, counterparties, categories, duplicates; the journal of changes with an
undo for every edit; reconciliation against the bank; rates from the ECB and
the Bank of Russia; a backup a day and an encrypted copy in Google Drive;
a screen lock; an MCP server over stdio, read-only.
