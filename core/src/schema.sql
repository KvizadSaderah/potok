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
