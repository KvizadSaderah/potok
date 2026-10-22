# potok — the specification

A local single-user ledger. One binary, SQLite on disk, a TUI as the main
interface. The goal: load the boring statements and see where the money goes
and who owes how much.

Written before the code. If the implementation disagrees with the
specification, it is the implementation that disagrees, until the
specification is corrected deliberately.

**Status:** a draft, 2026-10-16. The PDF spike is done, the result is below.

---

## 0. What this is not

Cut off deliberately rather than «for later»:

- a planner with hand-made templates and planned instances (`v2_planned_templates`, `v2_planned_instances`);
- savings goals (`v2_savings_goals`);
- complex allocations, carrying a budget's remainder between periods and «free money»;
- a command centre, insights, AI briefs;
- a web interface, mobile entry, several users.

The grounds for the first two are not taste. In saldo's live database a check
on 2026-09-04 established that `v2_planned_templates`, `v2_planned_instances`
and `v2_savings_goals` are **empty**. Three screens built and not used.

Instead of storing planning tables `potok` uses two simple things over the
actual data:
- a flat monthly ceiling on a category (`budget`, in the base currency's minor units);
- recognising regular charges automatically out of the history of operations (`analytics::regular`) and an estimate of the spending pace to the end of the month.

## 1. Where it lives and why here

A repository of its own, `potok`, since 2026-10-21.

This section used to say the opposite, and the reason it said so still holds
for the time it was written: a new project needs to read the old one for
months — the bank parsers, the fixtures, the functions that check the
invariants — and across a repository boundary that is friction at every step,
while an atomic commit touching both is impossible. It lived inside the
`saldo` repository as `potok/` for exactly that reason.

By the time it was split out, that reading was done: nothing here reaches for
saldo's sources any more, and what was taken from them is named in §10. The
split was `git subtree`-shaped and kept the history, which is what the old
section promised would always be possible.

## 2. Money

**Money is an `i64` in minor units plus a currency. The type is `Money(i64, Currency)`.**
Not `f64`, not `f32`, not a string, not a decimal library.

```rust
pub struct Money { minor: i64, currency: Currency }
```

Adding two `Money` of different currencies **does not compile**. That is not
style — in the old system summing different currencies into one heap was a
real bug, and the rule exists so that it becomes impossible rather than
unlikely.

### The rounding rule

**Rounding happens once, at the last step before the screen. In the middle of
a chain — never.**

Measured on the old system: float itself is harmless at these scales — over
ten thousand operations the discrepancy is 1e-7. But the difference between
«round every conversion and add» and «add and round» gave **1.63 € over a
thousand operations**. That is not an error of representing a number, it is an
error of where the rounding goes, and it reproduces in any language with any
type.

So `Money` has no rounding method. When a multi-currency basket is converted
for a total (`rate::convert_bag`, `rate::total`, `analytics::to_base`,
`budget::status`) the products `minor * rate_e9` are added up in `i128` (at the
scale of `10^9`), and the division with bankers' rounding to minor units
happens exactly once, on the final sum.

### Rates

A rate is an `i64` multiplied by 10^9 (`rate_e9`). A conversion is
`minor * rate_e9 / 10^9` in `i128`, rounded once on the final sum.

Looking a rate up for a date (`rate::on_or_before`) takes the last rate known
on that date or earlier (and failing that, within 3 days forward, since the
Central Bank of Russia publishes a rate dated the next working day). If there
is no rate for a currency in the database at all, the converted sum is not
replaced by zero: the currency comes back in the `missing` list and is shown
plainly (`no rate for <currency>`).

## 3. The invariants

Each is a check `potok verify` runs. The command either says «it agrees» or
names the row.

1. **An account's balance** = the opening balance + the sum of `balance_effect`
   of every posted operation of that account. Not stored. Computed.
2. **A debt's remainder** = the sum of the lendings − the sum of the
   repayments, write-offs and adjustments. **Not stored. Computed.** The
   original amount is the first lending and is not stored separately (the
   correction of 2026-10-18: the first revision added it to the movements and
   doubled every debt — see §4). Every debt movement must keep the debt's
   currency, and when tied to an operation (`operation_id`) must refer to a
   posted operation with the role `debt`.
3. **A transfer between one's own accounts** is exactly two legs with one
   `pair_id`, both with one role (`transfer` or `passthrough`) and the same
   `state`, with opposite signs of `balance_effect`. In one currency the two
   legs' `balance_effect` sums to zero; in different currencies the effects'
   signs are opposite. One leg without the other is a mistake, not an operation.
4. **An operation's currency equals its account's.** Checked by triggers on
   `INSERT` and on `UPDATE`. A cross-currency transfer is two operations in
   their own currencies plus a rate, not one operation with two currencies.
5. **`amount` is always positive.** The direction lives in the effect's sign:
   the amount is «how much», the effect is «which way». The effect equals the
   signed amount, with one named exception: a row with the role `excluded` may
   have a zero effect — a statement row a person told it not to count. (In
   saldo the adjustments were stored with a negative amount; on the move they
   get a positive amount and a negative effect — the same movement of money.)
   If an operation is split across categories (`operation_split`), the parts
   sum strictly to the operation's `balance_effect`.
6. **A pending operation is not money.** Nothing but `POSTED` enters any total,
   any balance or any flow. The Revolut statement says so itself: the balance
   in it counts the completed operations only.
7. **An operation created by an import refers to exactly one import row.**
   Importing the same file again creates nothing.
8. **A correction is not an edit.** The amount, the date and the account of a
   posted operation never change. A mistaken operation — a duplicate — is
   **removed**: the state `REVERSED`, it counts in no total and in no debt, but
   stays visible and comes back with one command. A transfer's legs are removed
   and brought back together only.

   The correction of 2026-10-18: the first revision required a counter-entry
   for a removed operation. That is wrong for a duplicate — a counter-entry on
   top of a removed one would count the money twice — and it disagreed with
   saldo's practice, where all 242 removed rows were a mark with no
   counter-entry. Found on a real duplicate: the 720 refund of 3 September was
   written in saldo twice out of two competing imports.

### Why 1 and 2 are not stored

In the old system a wallet's balance is recomputed by a trigger as a full sum,
while a debt's remainder is adjusted in parts (`remaining = remaining ± amount`)
and is **recomputed by nothing**. Two philosophies for one kind of number in
one schema. Because of that the sync refuses to write debts, and drift is found
only by a check.

Here both are computed. The class of bug disappears rather than being treated.

The price is a query instead of reading a column. For a personal ledger of tens
of thousands of rows that is an index and milliseconds. If one day it stops
being enough, a cache table is added **together with a check that it equals the
computation**, and never without one.

## 4. The SQLite schema

No `user_id` anywhere: one user, one file, no row-level security, no 48 tables
with policies. That is the largest saving against the old schema.

```sql
pragma journal_mode = wal;
pragma foreign_keys = on;

-- ── Accounts ───────────────────────────────────────────────────────────────
create table if not exists account (
  id                 integer primary key,
  name               text    not null unique,
  currency           text    not null,          -- ISO 4217, 3 letters
  kind               text    not null,          -- CASH | CARD | BANK | OTHER
  state              text    not null default 'ACTIVE',   -- ACTIVE | ARCHIVED
  inclusion          text    not null default 'FULL',     -- FULL | OFF_BUDGET | EXCLUDED
  opening_minor      integer not null default 0,
  created_at         text    not null default (datetime('now')),
  check (length(currency) = 3),
  check (state     in ('ACTIVE','ARCHIVED')),
  check (inclusion in ('FULL','OFF_BUDGET','EXCLUDED'))
);

-- ── Counterparties and their spellings ────────────────────────────────────
create table if not exists counterparty (
  id          integer primary key,
  name        text    not null unique,          -- the canonical name
  -- A counterparty's category is the categorisation rule itself.
  --
  -- There is deliberately no separate table of rules: mark a counterparty once
  -- and all of its operations, past and future, have the category, because it
  -- is read through coalesce and no backfill is needed. The decision has a
  -- ceiling: it cannot express «if the amount is over X» or «in this period
  -- only». A table of rules will arrive when that is needed, not before.
  category_id integer references category(id) on delete set null,
  -- When the name was given by hand. `refresh-names` leaves such a name
  -- alone: a person chose it rather than deriving it from descriptions.
  renamed_at  text,
  -- A note by hand: what the place the bank called «NORD OOD» actually is.
  -- Visible in a counterparty's details and in the CLI.
  note        text
);

create table if not exists counterparty_alias (
  alias           text    primary key,          -- as the bank writes it, normalised
  counterparty_id integer not null references counterparty(id) on delete cascade
);

-- ── Categories ─────────────────────────────────────────────────────────────
create table if not exists category (
  id        integer primary key,
  name      text    not null unique,
  parent_id integer references category(id) on delete restrict
);

-- ── Operations ─────────────────────────────────────────────────────────────
create table if not exists operation (
  -- autoincrement: the number of an operation deleted by a rollback does not
  -- go to a new one — else an old journal row would undo somebody else's edit
  -- (the audit of 2026-10-20).
  id               integer primary key autoincrement,
  account_id       integer not null references account(id) on delete restrict,
  role             text    not null,   -- income|expense|transfer|passthrough
                                       -- |debt_movement|opening_balance
                                       -- |correction|excluded
  state            text    not null default 'POSTED',  -- POSTED|PENDING|REVERSED|CORRECTED
  occurred_at      text    not null,              -- ISO 8601
  amount_minor     integer not null check (amount_minor > 0),
  currency         text    not null,
  effect_minor     integer not null,              -- the sign is the direction
  description      text,
  counterparty_id  integer references counterparty(id) on delete set null,
  category_id      integer references category(id)     on delete set null,
  pair_id          integer references operation(id)    on delete restrict,
  reverses_id      integer references operation(id)    on delete restrict,
  import_row_id    integer references import_row(id)   on delete restrict,
  -- An own note: «return to Kolya», «this is for the repair». The bank writes
  -- description, a person writes this. Editable always, unlike amount and date.
  note             text,
  created_at       text    not null default (datetime('now')),
  check (role  in ('income','expense','transfer','passthrough',
                   'debt_movement','opening_balance','correction','excluded')),
  check (state in ('POSTED','PENDING','REVERSED','CORRECTED')),
  -- The effect equals the signed amount. There is one exception and it is
  -- named: a row with the role excluded may move no balance at all. That is
  -- what a statement row a person told it not to count looks like — saldo has
  -- four, of 800.00 and 769.90. Throwing them away is not allowed: the next
  -- import of an overlapping statement would bring them back as a spend.
  check (abs(effect_minor) = amount_minor or (role = 'excluded' and effect_minor = 0))
);

create index if not exists operation_by_account_date on operation(account_id, occurred_at);
create index if not exists operation_by_date         on operation(occurred_at);
create index if not exists operation_by_category     on operation(category_id);
create index if not exists operation_by_counterparty on operation(counterparty_id);

-- An operation's currency equals its account's — invariant 4, in the database.
create trigger if not exists operation_currency_matches_account
before insert on operation
for each row when (select currency from account where id = new.account_id) <> new.currency
begin
  select raise(abort, 'operation currency does not match its account');
end;

create trigger if not exists operation_currency_matches_account_update
before update of account_id, currency on operation
for each row when (select currency from account where id = new.account_id) <> new.currency
begin
  select raise(abort, 'operation currency does not match its account');
end;

-- ── Debts ──────────────────────────────────────────────────────────────────
-- A debt has no original amount. It is the first ISSUE movement, and storing
-- it once more would mean a second copy of the same number, which one day
-- disagrees with the first. saldo is built the same way:
-- v2_verify_debt_movements counts the remainder as ΣISSUE − Σthe rest and does
-- not touch an original amount. The first revision of this schema added the
-- original amount to the movements, and on the real data every debt would have
-- come out exactly twice as large. Found by reading saldo's formula before the
-- move, not by a check after it.
create table if not exists debt (
  id              integer primary key,
  counterparty_id integer not null references counterparty(id) on delete restrict,
  direction       text    not null,            -- OWED_TO_ME | I_OWE
  currency        text    not null,
  opened_on       text    not null,
  due_on          text,
  note            text,
  check (direction in ('OWED_TO_ME','I_OWE'))
);

create table if not exists debt_movement (
  id           integer primary key,
  debt_id      integer not null references debt(id) on delete cascade,
  operation_id integer references operation(id) on delete set null,
  kind         text    not null,               -- ISSUE|REPAYMENT|WRITE_OFF|ADJUSTMENT
  amount_minor integer not null check (amount_minor > 0),
  occurred_on  text    not null,
  note         text,
  check (kind in ('ISSUE','REPAYMENT','WRITE_OFF','ADJUSTMENT'))
);

-- ── The import: a file is kept whole ──────────────────────────────────────
create table if not exists import_file (
  id          integer primary key,
  sha256      text    not null unique,     -- idempotency by content
  bank        text    not null,            -- revolut | icard | …
  filename    text    not null,
  archived_at text    not null,            -- the path in the archive beside the database
  imported_at text    not null default (datetime('now')),
  account_id  integer references account(id) on delete set null  -- onto which account
);

create table if not exists import_row (
  id        integer primary key,
  file_id   integer not null references import_file(id) on delete cascade,
  line_no   integer not null,
  raw       text    not null,              -- the original row as it was
  signature text    not null,              -- the dedup key
  pending   integer not null default 0,    -- pending: not money yet (inv. 6)
  unique (file_id, line_no)
);

create index if not exists import_row_by_signature on import_row(signature);

-- ── Rates ──────────────────────────────────────────────────────────────────
create table if not exists rate (
  on_date  text    not null,
  base     text    not null,
  quote    text    not null,
  rate_e9  integer not null check (rate_e9 > 0),
  source   text    not null,               -- ecb | manual | …
  primary key (on_date, base, quote)
);

-- ── Reconciling with the bank ─────────────────────────────────────────────
-- The remainder a person saw at the bank, at the end of a day. It is an
-- observation rather than a correction: the balance is still counted from the
-- operations, and the reconciliation only says whether it agrees. The gap is
-- recomputed on every show — remove a duplicate and it shrinks at once, with
-- no new reconciliation.
create table if not exists reconciliation (
  id          integer primary key,
  account_id  integer not null references account(id) on delete cascade,
  on_date     text    not null,               -- YYYY-MM-DD
  bank_minor  integer not null,               -- what the bank showed
  created_at  text    not null default (datetime('now'))
);

-- A group of similar operations a person said «this is not a duplicate» about:
-- two fees of 0.50 on one day do happen. The key is the operation numbers
-- comma-separated and ascending; if a third joins, that is another group and
-- it will be shown.
create table if not exists duplicate_dismissed (
  operation_ids text primary key,
  created_at    text not null default (datetime('now'))
);

-- The journal of edits: what was changed by hand, when, and how to undo it.
-- Written in the same transaction as the edit. `undo` is the opposite action
-- in words for `journal::undo`; empty means the edit does not undo (a merge of
-- counterparties).
create table if not exists change_log (
  id         integer primary key,
  at         text    not null default (datetime('now')),
  what       text    not null,
  undo       text,
  undone_at  text
);

-- The ledger's settings: the language, the statements folder, the account for
-- entering by hand, the flows period. Key — value; which keys there are and
-- their defaults is `settings.rs`. They live in the ledger itself rather than
-- a file beside it: a copy of the ledger opens as it was left.
create table if not exists setting (
  key    text primary key,
  value  text not null
);

-- Splitting an operation across categories: a receipt of 42.50 — 30 «Food»,
-- 12.50 «Household». The parts sum to the operation's amount (edit::split
-- holds that); the flows take the parts instead of the operation itself.
create table if not exists operation_split (
  id           integer primary key,
  operation_id integer not null references operation(id) on delete cascade,
  category_id  integer not null references category(id) on delete restrict,
  amount_minor integer not null check (amount_minor > 0)
);
create index if not exists operation_split_by_operation on operation_split(operation_id);

-- The budget: a monthly limit on a category, in one currency. How much was
-- spent the flows count; only the limit is stored here.
create table if not exists budget (
  category_id  integer primary key references category(id) on delete cascade,
  currency     text    not null,
  limit_minor  integer not null check (limit_minor > 0)
);

-- Correcting the accounting month's start by hand: the rule (a day of the
-- month or the last working day) knows no holidays. The key is the calendar
-- month the rule's start belongs to (`YYYY-MM`).
create table if not exists month_start (
  month  text primary key,
  start  text not null
);

-- ── Computed, not stored ──────────────────────────────────────────────────
create view if not exists account_balance as
select a.id            as account_id,
       a.currency      as currency,
       a.opening_minor + coalesce(sum(o.effect_minor), 0) as balance_minor
from account a
left join operation o on o.account_id = a.id and o.state = 'POSTED'
group by a.id;

create view if not exists debt_remaining as
-- The same formula as in saldo: lendings add, everything else — a repayment,
-- a write-off, an adjustment — subtracts. An adjustment that increases a debt
-- is written as an ISSUE: that way a movement's amount always has one sign.
select d.id as debt_id, d.currency,
       coalesce(sum(case when m.kind = 'ISSUE' then m.amount_minor else 0 end), 0)
         as issued_minor,
       coalesce(sum(case when m.kind = 'ISSUE' then m.amount_minor
                         else -m.amount_minor end), 0)
         as remaining_minor
from debt d
-- A movement whose operation is removed (a duplicate) does not enter the debt:
-- that way a duplicate tied to a debt as a «repayment» stops lowering it, and
-- comes back if the operation comes back. A movement with no operation always
-- counts.
left join debt_movement m on m.debt_id = d.id
  and not exists (select 1 from operation o
                   where o.id = m.operation_id and o.state <> 'POSTED')
group by d.id;
```

### The schema was checked by running it, not by proofreading

On 2026-10-16 this block was cut out of the file and run through `sqlite3`. It
creates with no errors — SQLite accepts `operation.import_row_id` referring
forward to a table declared below. Checked on inserts:

- `account_balance` gives 395.88 with an opening balance of 402.12 and a spend of 6.24;
- an operation in state `PENDING` **did not** enter the balance — invariant 6 is
  held by the database itself rather than by code;
- the currency trigger rejected an operation in USD on an account in EUR;
- `check (abs(effect_minor) = amount_minor)` rejected an amount of 1.00 with an effect of −9.99;
- `debt_remaining` gives 550.00 from an original 800.00, a repayment of 300.00
  and a lending of 50.00, and gives the original amount for a debt with no
  movements.

One behaviour is worth knowing beforehand: deleting a category is not
forbidden, it clears `category_id` on the operations (`on delete set null`).
The restriction stands on `category.parent_id` alone, so as not to orphan the
tree.

### The schema's version — `pragma user_version`

Added 2026-10-18. The schema was applied through `create … if not exists`, and
a database from an earlier potok quietly kept its own schema. Exactly such a
file turned up: a real ledger from 16 October, empty but holding a
`debt_remaining` view that doubled the debts. `create view if not exists` does
not replace such a view — the new code would have counted by the old formula,
silently.

Now the version is written in the file (`db::SCHEMA_VERSION`, 9 at present),
and a file of another version is not opened at all — neither for writing nor
for an agent to read. The refusal says whether the file is empty: an empty one
can be deleted, one with data has to be moved over again. From version 1
onwards every change comes with a migration of its own (`db::MIGRATIONS`).

The correction of 2026-10-20 (the audit): opening a file of its own version
writes nothing. Before, every open took a write lock in order to apply the
schema again, and the background send to the cloud, having opened the same
file, refused the command its write: 96 of 133 `potok add` in a row —
«database is locked». Version 7 is `operation.id` with `autoincrement`:
rolling a statement back deleted operations, their numbers went to new ones,
and an old journal row undid an edit of somebody else's operation. The
migration rebuilds the table with the foreign keys off and checks them
afterwards (`pragma foreign_key_check`).

**The archive of originals.** Beside the database — `archive/<sha256>.<ext>`,
the original statement file as it was. That is what the old system lacks, and
it guards against its known weak spot: the dedup key depends on the
description, and improving the cleanup of descriptions re-opens the history to
duplicates. With the archive the parser can be improved and everything run
again from scratch.

## 5. The boundary between the core and the TUI

Two crates, one shipped binary.

```
potok-core/   the domain, SQLite, the import, the queries — not a line about a terminal
potok/        ratatui, built into one executable
```

**The rule that must not be broken:**

> The TUI runs not one SQL query and makes not one money computation. It gets
> finished values and draws them. Everything `potok-core` can do is expressed
> by types rather than by strings.

That is the same wording as in `tui/src/api.rs` of the old client, and there it
proved itself. The difference is that on the other side of the boundary here
there is no network but a library in the same process.

There are exactly two reasons, both practical:

1. **Checkability.** The money rules are tested with no terminal. The TUI is
   tested by drawing into a buffer — the same way as in `saldo` now, and there
   it works.
2. **Without it no surface for an agent is possible.** An agent presses no keys.

The subcommands are not a second interface but the same library with another
output. So they are nearly free and need no separate design.

### What this interface does not have, and that is the main gain from local-first

Found out while building, 2026-10-16. The database is a file in the same
process, reading is instant. So the interface has **no asynchrony at all**: no
`Loadable`, no «loading…», no cache of the last answer, no timestamp of when a
figure was true, no three screens that live without a server against ten that
are empty without one.

In the old client all of that is most of `app.rs` and the whole of `cache.rs`,
and it exists only because a network lies between the screen and the figure.
Take the network away and it goes too. The screens read directly, and the
redraw happens on every turn of the loop.

### `potok screens`

Draws every screen into text of a given size. A tool for the eyes: in the old
client looking at all the screens at 150x44, 100x30 and 80x24 found most of
the layout defects — «found this way rather than reasoned out», as it is
written there. The tests check that a screen draws; this shows how it looks.

```
potok                      opens the TUI — the main path
potok import <file>        parse it and offer it for confirmation
potok verify               check every invariant of section 3
potok flows --period …     where the money went
potok mcp                  the MCP server over stdio
```

~~Every reading subcommand has a `--json`.~~ Not done and not needed: the
machine entrance is MCP (§6), and it gives the same numbers as the screens.
The correction of 2026-10-20.

## 6. The MCP surface

`potok mcp` is an MCP server **over stdio**. No ports, no OAuth, no tokens: the
agent runs as a child process and inherits the rights to the database file. In
the old system that was HTTP with OAuth over 786 lines, because the server was
in a cloud; locally it is surplus.

**Reading is open. Writing is off and is turned on by the `--allow-writes` flag.**

The reason: in the cloud version the agent was held back by the database's
row-level security, it saw only its own rows. Locally there is no security — it
has the whole ledger. An agent that posts operations in your ledger itself must
not appear by accident.

The reading tools (the names are taken from `src/lib/mcp/server.ts` in `saldo`,
they are already worn in):

| tool | what it gives |
|---|---|
| `get_financial_overview` | the balances, what went out and in over a period by currency, the flows, the open debts. This month by default |
| `list_accounts` | the accounts and their computed balances |
| `search_operations` | operations by text, account, category, dates; a `truncated` flag if more was found than given |
| `list_debts` | the debts and their computed remainders |
| `list_counterparties` | who, how many operations and for how much; the unmarked ones alone if wanted |
| `list_imports` | which statements were loaded and how many rows they gave |
| `verify` | the result of checking the invariants |

There is one difference from the original list and it is not cosmetic:
`list_pending_imports` became `list_imports`. In `potok` there are no
unconfirmed imports — an import is written whole or not written at all (§7) —
so a tool with that name would answer a question that does not exist here.

Under `--allow-writes` these will be added: `import_file`,
`categorize_operations`, `set_counterparty_alias`, `create_operation`,
`create_transfer`. The flag does not exist at all yet: it will arrive together
with the first writing tool, not before.

### Done 2026-10-16 — how and why this way

**The ban on writing is held by the database, not by the list of tools.** The
server opens the ledger through `db::open_read_only`, where `pragma query_only`
is on: any `insert`, `update`, `delete` or `drop` on that connection fails
inside SQLite, whatever function was called. The absence of a registered
writing tool is a weak guarantee: what does not exist today is added in one
line tomorrow. A test runs every tool on such a connection — that is how it is
checked that not one of them writes quietly.

The `SQLITE_OPEN_READ_ONLY` flag would be stricter, but on a database in WAL
mode it does not open while there is no `-shm` file beside it. `query_only`
works whatever state the file is in.

A database that does not exist the server does not create: an agent connecting
before the first import gets a refusal with a hint rather than a quietly
created empty ledger.

**Money going out is a string and whole cents, never `f64`.**
`{"amount": "-6.24", "minor": -624, "currency": "EUR"}`. A floating-point
number an agent reads as `4873.4` and adds to another like it — the first
mistake in the figures. A test walks every tool's answer and fails on any
fractional number. The totals by currency in the overview go separately, and
the core counts them (`query::flow_totals`), not the server: rule §5 holds for
the agent's surface too.

**A crooked date is an error, not an empty answer.** SQLite does not complain
about `date('yesterday')` but returns NULL, and the search would quietly give
no rows — an agent would read it as «there are no operations». The dates are
checked before the query.

**The protocol is written by hand on `serde_json`**, with no official Rust SDK.
The SDK pulls in tokio and asynchrony, while the binary is synchronous
throughout — an interface without a single `async` was the main gain of
local-first (§5), and dragging a runtime in for seven tools would bring back
what was deleted. The subset needed is small: `initialize`, `ping`,
`tools/list`, `tools/call`, the notifications.

The protocol version is the one the client asked for, if it is out of
`2025-11-25`, `2025-06-18`, `2025-03-26`, `2024-11-05` (the list of SDK
1.30.0); otherwise ours, the latest. The tools have one shape in all four.

**Compatibility was checked with the official client**
`@modelcontextprotocol/sdk` 1.30.0 — the one `saldo`'s MCP server is written on
— on real statements: the handshake, all seven tools, a refusal on an unknown
one. September through the agent — 8427.93 EUR out, 8926.59 EUR in, exactly
what `potok flows` says.

Connecting it to Claude Code:

    cargo install --path potok/cli
    claude mcp add potok -- potok mcp

Never added: deleting, editing a posted operation, writing a debt off. That is
a reversal and a person.

## 7. The import

### The spike is done, 2026-10-16

Checked on real statements: `account-statement_2026-09-01_2026-10-16` (Revolut,
12 pages) and `transactions16_10_2026` (iCard, 6 pages).

**The pure Rust crate `pdf-extract` 0.9 read both files.** Outside programs
(`pdftotext` from poppler) are not needed — they were checked, they work, but
they are not required.

The Revolut result, as Rust gives it:

```
operations     : 198
balance chain  : 198 of 198
out            : 4873.40   in the statement 4873.40   agrees
in             : 4557.42   in the statement 4557.42   agrees
total          :   86.14   in the statement   86.14   agrees
```

The parsing is one regular expression per line of the form
`Sep 1, 2026 Description €23.67 €344.25`.

On iCard: 87 operations. **The correction of 2026-10-16, the same day:** the
first revision of this paragraph asserted that every iCard operation is one
line with all the fields. That is untrue. The extracted text tears a record
where the PDF wrapped, and only 13 records out of 87 came out whole; the
paragraph was written from the lines that happened to catch the eye in a
sample.

The real shape is a record of several lines which **begins** with a line of the
form `HH:MM DD.MM.YYYY` and **ends** with a line whose last word is a status.
On the real statement there are 87 such beginnings and 87 such ends, so the
marks are paired and no record is lost. The lines between them are glued with
a space and parsed as one.

It is worth writing down how this was found: a parser written from the wrong
paragraph passed every test on the sample and **quietly lost 74 operations out
of 87**. A run on the real file caught it, not a test. Hence the rule for the
other banks: a sample for the tests is made from the real output, not from how
it was remembered.

The spike's working files are in the session's scratchpad, outside the
repository: real statements do not reach it and cannot.

Another 7 Revolut rows did not reach the 198 and **should not have**: that is
the pending section, and they have no balance column. The mark of a pending
operation is structural rather than textual: no balance in the row.

### A check on every row, not on the total alone

Revolut prints the running balance in every row. That gives a check **on every
operation**: `balance[n] − balance[n−1] == ±amount[n]`.

That is stronger than a check on a batch's total, and it has to be a
first-class function of the import. In the old system the balance check on a
Revolut statement degenerates into a tautology — real drift shows as green.
Here the arithmetic the bank printed is what is checked.

iCard has no balance in the row and no totals in the document. For it the check
is only comparing the account balance obtained with what the bank's app shows,
by hand. That has to be said on screen rather than passed off as checked.

### Idempotency

The key is the file's `sha256`. The same file a second time creates nothing.
Inside a file a row is identified by the pair `(file_id, line_no)` rather than
by the description's text. The content's signature (`signature`) is used for
finding duplicates **between** files — overlapping statement periods.

### Reconciling with the bank and similar operations — 2026-10-18

A signature catches duplicates between the files of one import. It did not
catch nine copies that came from saldo, where one iCard statement was loaded
twice in different description formats. The debit card showed −1 324.68, and a
person noticed it rather than the program: iCard prints no balance, so the
import had nothing to check against.

So there are two things that only show rather than fix (`reconcile.rs`):

* **The reconciliation.** A person types the remainder from the bank on a date
  (`s` on an account or `potok reconcile`). An observation is stored, not a
  correction. The gap is recomputed on every show, so when you remove a
  duplicate you see it shrink at once. Beside it is shown what the balance
  would be without the similar operations, and the operations whose amount
  equals the gap.
* **Similar operations** (the «Duplicates» screen, `potok duplicates`). One
  account, one day, one signed amount and one counterparty, and without a
  counterparty the description. The role is not in the key: one copy of a
  top-up was a transfer, the other a spend. These are candidates, not a
  verdict. Two fees of 0.50 on one day are real, and «not a duplicate» hides a
  group until a third operation joins it.

### Parsing before writing — 2026-10-18

`import::review::plan` writes nothing. It says of every statement row whether
it is new, a duplicate or pending, and to the new ones it adds a hint and
marks. `store::apply` writes what was decided.

* **A transfer.** On another account there is already a posted operation tied
  to nobody, in the same currency, for the same amount with the opposite sign,
  no further than three days off. The same search as «find the second leg» in
  the menu (`edit::partners`). Two rows do not share one operation.
* **A debt.** It is the row's counterparty's debt, or a debt this counterparty
  has already paid on. The second is required: the Northwind debt in the real
  ledger has the counterparty «Northwind» while its payments have «NORTHWIND»
  from the statement. Out on an «I owe» debt is a repayment, in is borrowing
  more; on «owed to me» it is the other way round.
* **The marks** only warn: a date later than today (such a row is not written
  by default), an amount larger than usual for the account at a counterparty
  that has seen no such sums, a transfer to a person with no debt.

From the command line every hint is accepted. From the interface a person drops
a row or a hint. The plan may have gone stale while it was looked at: if the
second leg was tied to something else in the meantime, the whole import rolls
back.

**Rolling a load back** (`potok unimport`, the menu on the import screen)
deletes its operations rather than marking them: it is the undo of a write just
made, the original lies in the archive, and the same file loads again after the
rollback. The second leg on another account stays and becomes an ordinary spend
or income. The move from saldo does not roll back this way: it is the whole
ledger.

### By hand — 2026-10-18

An operation entered by hand (`edit::add_operation`) is like an imported one,
only with no statement row. A spend or income is decided by the sign, the
counterparty by «who», and an own category is set if one was named. It is made
a transfer or a debt by the same menu as any other.

«Correct it» (`edit::replace_operation`) does not change a posted operation in
place but removes it by a mark and enters a new one, in one transaction. The
old one stays visible and comes back. A transfer's leg and a debt movement
cannot be corrected this way: they have a second side.

If an operation entered by hand is later brought by a statement, the import
will not recognise it: there is no signature, there is nothing to compare. If
«who» is named so that it reduces to the same counterparty as the statement
row, both will turn up on the «Duplicates» screen: one account, one day, one
amount, one counterparty. If not, they show only when reconciling with the
bank. Cash is untouched by this: it has no statement.

### Debts by hand — 2026-10-18

`debt.rs` opens a debt, ties an operation to it and writes the remainder off.
Only the movements are written, the remainder is counted by `debt_remaining`
(invariant 2).

* **Opening** is possible with an amount and no operation (an old debt, in
  cash, from before the accounting began) or from an operation, which becomes
  the first lending. A friend sent 300 — you open the debt from that operation.
  An empty debt, with no amount and no operation, is refused.
* **The kind of movement** is decided by the operation's sign and the debt's
  direction: «I owe» — out means paid back, in means borrowed more; on «owed to
  me» the other way round. The import's hint uses the same rule
  (`debt::kind_for`).
* **Refusals:** a repayment larger than the remainder (that is not this debt),
  another currency, a transfer between one's own, an operation already tied, a
  removed operation.
* **Writing off** is a `WRITE_OFF` movement for the whole remainder with no
  operation: there was no money.

### The journal of edits — 2026-10-18

Every edit in the core writes a row into `change_log` in the same transaction
as the edit itself (`journal::log`): remove, bring back, untie, tie or break a
transfer, an own category, entering and correcting, an opening balance, a debt
(open, tie, write off), «not a duplicate», a reconciliation, a counterparty's
category, loading a statement and rolling it back. So the journal is complete
wherever the edit came from.

Undoing is the opposite action through the same functions (`journal::undo`),
not putting rows' old values back. The opposite action checks what the edit
checked and is itself written to the journal, so an undo can be undone too. If
something has changed since (the operation that has to be tied to the debt
again has already been removed), the undo refuses with a reason.

Not undoable, and the journal shows it: a merge of counterparties, renaming
counterparties, rolling a statement back (it is loaded again), deleting a debt
and the other records of undos themselves, which have no opposite action. The
time is stored in UTC and shown local.

Undoing a counterparty's category gives its previous category back but does
not give back the operations' own categories that matched the new one and were
stripped at the marking (§7a).

### How an operation counts — 2026-10-18

`edit::set_counting` switches a spend or income, «not counted» (`excluded`) and
transit (`passthrough`). The balance does not change with it: the money on the
account is the same, only whether the operation enters the flows changes. Only
a spend and income enter the flows (`FLOW_FILTER`). «Count it again» sets a
spend or income by the sign. A row with no sign (a zero effect from saldo)
cannot be brought back that way: out or in is unknown, and the schema permits a
zero effect for «not counted» only. A transfer and a debt movement change by
their own edits: they have a second side.

**A refund — 2026-10-20.** Money that came back for a purchase is not income
but the undoing of a spend: give the trainers back for 100 and you spent 0, not
«spent 100 and earned 100». Count a refund as income and the remainder and the
month's difference stay the same, but «out» and «in» are inflated by its
amount, the shop's category shows a spend that is not there, and the budget
fires for nothing. YNAB and Firefly III are built the same way: a refund lowers
the spend of its own category.

A refund is stored as a spend with a plus sign (`edit::Counting::Refund`,
`edit::kind` calls it `refund`); the schema needs no new role. In the flows it
is subtracted from its row's «out» and does not enter «in»; nor does it in a
day's spends. It is recognised as a refund by the words
(`counterparty::refund_of`: «Reversal of …», «Provisional refund for … claim»,
«Refund for/from …») rather than by the counterparty: money from a shop is
sometimes not a refund, and money from a person never is one (a debt, a gift).
The seller is taken from the refund's text, and the refund lands in the shop's
category; the dedup signature does not change because of it. A refund that came
in another month is subtracted in the month the money arrived: closed months
are not changed after the fact. The import writes a refund at once; older rows
are converted by `potok refunds apply` or by `x` on the import screen — each as
a journal row, undoable.

### Counterparties by hand — 2026-10-18

* **Rename** (`counterparty::rename`). A name given by hand is marked with
  `renamed_at`, and `refresh_names` leaves it alone: a person chose it rather
  than it being derived from descriptions. If the name is taken, the rename
  refuses: such counterparties are folded, not renamed.
* **«This is another counterparty»** (`counterparty::reassign`). An operation's
  spelling, that is its normalised description, starts to mean another
  counterparty (an existing one or a new one). Every operation of the previous
  counterparty with the same spelling moves with it, and the next import knows
  it at once. Undoing gives back both the operations and the spelling's
  previous meaning.
* **Fold** (`counterparty::merge`). From the interface — by part of a name,
  choosing among those found. There is no undo, and the journal shows it.

### 2026-10-19: towards self-sufficient

The new core modules and what they hold:

* `backup` — `VACUUM INTO`, once a day on its own, 30 copies.
* `account` — create, rename, archive only at zero.
* `category` — list, rename, fold, delete; touches the counterparties, the
  operations' own categories and the parts of splits at once.
* `budget` — only the limit is stored, what was spent comes from the flows.
* `rate` — rates by hand, a total in the main currency, a missing rate is
  named rather than guessed.
* `analytics` — the months and the dynamics through `query::flows` (one rule
  for everyone), the regular payments (monthly or weekly, amounts within 10%,
  the ceased ones not shown), the overview.
* `export` — CSV, the sums as a decimal string.
* `edit::split` — the parts instead of the operation in the flows;
  `edit::add_transfer` — a transfer by hand; a pair across currencies requires
  the opposite sign only.
* The accounting month from payday: `analytics::cycle_start`; with day 1 that
  is the calendar month, and the old tests hold it.

`period` — the accounting month: from a day of the month or from the last
working day (it knows the weekends, not the holidays), a month's correction by
hand in `month_start`, in the journal and undoable. `rate::update` — the ECB
and the Central Bank of Russia through `curl`, one journal row per update; an
unreadable row of a feed is skipped rather than dropping the whole feed (the
Central Bank writes tiny rates in exponent form).

The schema: `operation_split`, `budget`, `setting`, `month_start`,
`counterparty.renamed_at`, `import_file.account_id`, `import_row.pending` — the
columns are added by `db::ensure_column` as the fact stands, the tables by
`if not exists`.

## 7a. Counterparties and categorisation

Added 2026-10-16, after stage 3.

A description in a statement is not the seller's name but what the processing
wrote. One shop arrives like this:

    Revolut:  Kaufland
    iCard:    POS Payment *1234, KAUFLAND BULGARIA SOFIYA BGR

Normalisation reduces both to the key `kaufland`, and it is one counterparty.
What is stripped: the operation type (`POS Payment`, `Transfer to`,
`Outgoing bank transfer` — in a loop, because two come in a row), the card mask
(including one glued to the name: `revolut**5678*`), the amount and currency
from the text, the legal form, the city and the country code, the shop's
numeric codes, the ligatures from a PDF (`Soﬁa` as a single glyph).

**If the description holds an IBAN, it alone becomes the key.** That settles
two cases that words cannot:

* a transfer's description carries a memo with a date — «rental payment for
  09.2026» — and next month it is another one. By words that gave a new
  counterparty every month;
* `НОРТУИНД БЪЛГАРИЯ ЕООД` and `NORTHWIND BULGARIA EOOD` are one company in
  two alphabets. Their IBAN is shared, and they come together on their own.

What normalisation **cannot** do: `dm drogerie` and `DM BULGARIA EOOD` stay
apart. For that there are `potok counterparty merge` and `potok alias add` —
folded once, it holds from then on. Pretending it is automatic would be worse
than saying so plainly.

On the real statements: 285 operations from two banks gave 88 counterparties,
and Kaufland gathered 23 operations from both banks, BILLA 27, Lidl 9.

### The categorisation rule is a column, not a table

`counterparty.category_id`. Mark a counterparty once and all of its
operations, past and future, have the category, because it is read through
`coalesce` at reading time rather than written into the rows. No backfill, no
recomputation.

The decision has a ceiling and it is named: it cannot express «if the amount is
over X» or «in this period only». A table of rules will arrive when that is
needed, not before.

`operation.category_id` is left for an exception on one operation and takes
priority over the counterparty's category.

## 8. The stages

| | what | done when |
|---|---|---|
| 0 | the PDF spike | **done** |
| 1 | the core: the schema, the Revolut and iCard imports, `verify`, `flows` | **done** |
| 2 | the TUI: accounts, operations, flows, debts, the import | **done** |
| 3 | the categories and rules, the counterparties' aliases | **done** |
| 4 | MCP over stdio, read-only | **done** |
| 5 | **moving the data over from `saldo`** | **done** — §9 on the real dump, see below |
| 6 | the rates, the accounting periods | the multi-currency totals |
| 7 | syncing two nodes | take `tui/src/sync.rs` whole, the model there is right |

**Stage 1 is already reaching the goal.** If loading statements and looking at
the flows work from the command line, the rest is convenience.

## 9. The criterion for switching off `saldo`

The first revision described this criterion and **provided for no stage at
which the data moves over** — as though it would be in place by itself. Stage 5
was added 2026-10-16.

Everything moves: the accounts with their opening balances, the operations with
all their fields and ties, the transfer pairs, the debts and their movements,
the counterparties with their aliases, the categories. The source is a dump of
the hosted database rather than a live connection: the move has to be
repeatable and not depend on whether production is reachable this minute. The
debts move **as movements, not as a remainder** — a remainder in `potok` is
computed, and copying it would be exactly the mistake being left behind.

The criterion itself, mechanical, with no «seems to work»:

1. take a dump of the live `saldo` ledger;
2. load it into `potok`;
3. compare **every** account balance and **every** debt remainder.

It agrees — we switch. It does not — it is not ready. The old system runs all
this time and stays the source of truth until that moment.

## 9a. The move is done — 2026-10-18

`potok migrate <dump> --user <the start of user_id> [--dry-run]`. The source is
a dump from `.github/workflows/backup.yml`, the one GitHub takes off Supabase
every night; the move needs no access to production.

**The result on the real dump:**

    operations moved:        1774
    rollbacks not moved:     242 (241 are import rollbacks; they enter no saldo balance)
    transfer legs in pairs:  118
    debts: 3, movements on them: 27

    iCard     saldo stores  −2 044.68 EUR   potok  −2 044.68 EUR   agrees
    Cash      saldo stores       0.00 EUR   potok       0.00 EUR   agrees
    Revolut   saldo stores      63.35 EUR   potok      63.35 EUR   agrees
    Tinkoff   saldo stores       0.00 RUB   potok       0.00 RUB   agrees
    the debts: three of three agree; potok's invariants agree.

And the check that decides whether this can be used from here on: over what was
moved, the same two PDF statements saldo once imported were loaded.
**Revolut — 198 of 198 recognised as duplicates, iCard — 87 of 87, not one new
operation, the balances did not move.**

### What was found on the way, and why it is written down

Each of these would have looked like «the move is broken» at the check, and
each was found by reading the data or by measuring, not by reasoning:

1. **The debt formula in the schema's first revision doubled every debt.** The
   original amount was added to the movements, while in saldo it is itself the
   first ISSUE movement (`v2_verify_debt_movements`: ΣISSUE − Σthe rest).
   Corrected in §3 and §4 before the move. The column for the original amount
   was removed: it would have been a second copy of the same number.
2. **saldo holds rows that potok's schema rejected.** Four «excluded» rows with
   a zero effect (of 800.00 and 769.90 — statement rows a person told it not to
   count) and two adjustments with a negative amount. The first got the named
   exception in invariant 5, the second move over as a positive amount with a
   negative effect.
3. **The time in the dump is `+00` with no minutes**, and SQLite's `date()`
   returns NULL on it: the operations would have fallen out of every date
   query. It moves as `…T…Z`; a test holds the very fact that SQLite does not
   understand it.
4. **The first measurement of the dedup failed: 200 of the statements' 285 rows
   doubled.** All two hundred had an operation in what was moved with the same
   account, date and amount — only the description differed. saldo glued
   Revolut's service lines into the description
   (`Kaufland To: Kaufland Bulgaria, Sofiya Card: …`), while potok takes the
   first part only. The labels are taken from the data — `To:` ×917, `Card:`
   ×847, `From:` ×129, `Reference:` ×127, `Fee:` ×3 and the rate line
   `Revolut Rate =` ×30 — and normalisation cuts a description at the first of
   them. iCard has the payment types saldo stripped (`ATM Deposit`,
   `Cash withdrawal`), a type that is the whole description, and the asterisks
   of an already cleaned card mask. Three runs: 13 → 197 → 198 of 198.
5. **The dedup signature now goes by the counterparty's key and the unsigned
   amount** — as in saldo, and for the same reason. That also cut the
   counterparties after the move from 438 to 300: one Kaufland with different
   «To:»s and card numbers stopped falling apart into dozens.

### What was deliberately not moved

* **REVERSED** — 242 rows, 241 of them import rollbacks. They enter no saldo
  balance; in the dedup signatures they would make a future import skip real
  rows as «already seen».
* **The currency rates** (4020 rows) — stage 6. **The automation rules** (2) —
  categorisation here lives on the counterparty.
* **Everything but one user.** saldo's database has three; the second is a test
  account with a fake wallet of 10 million. A prefix that fits two is a refusal,
  not a guess.
* **The dump itself** is not put into the archive: it holds the `auth` tables
  with password hashes.

### The categories

A counterparty's category is set only if in saldo all of its operations were in
it, including having none — this is a move, not a guess. That gave 98
counterparties with a category whole. The remaining operations (530) have their
own category, as it was in saldo; such a counterparty is marked later with one
command — and the category goes to exactly those of its operations that had
none.

### The known ceiling

The dedup between files asks «is there such a signature», not «how many
times». Two identical purchases on one day at one price in a new statement
against one in the ledger — the second will be skipped as a duplicate. saldo
matched by count. On this data the case did not come up (198 of 198 with no
extra skips, the balances agreed to the cent), but it is possible.

## 10. What to take from `saldo`

**Copy as it is (Rust):** `fmt.rs`, `theme.rs`, `lock.rs`, `config.rs`,
`sync.rs` whole for stage 6, the fixtures with anonymised statements.

**Take the pattern, trim the list:** `action.rs` — every action is declared
once, the keyboard, the menu and the help read one list, and tests hold that
across every screen. From `ui.rs` and `form.rs` — drawing a field, the borders,
clipping by width, the scroll window with the cursor, together with their tests.

**Read as a specification, write anew:** 3707 lines of the import's brain in
TypeScript, `revolut.ts` 459 and `icard.ts` 420 among them — the accumulated
knowledge of what these statements look like. Plus the `v2_verify_*` functions
— ready-made wordings of the invariants.

**Do not take:** `cache.rs` — the local store became the source. `models.rs` —
wire shapes with `f64`, read as a catalogue of fields. `secrets.rs` — only with
stage 6.
