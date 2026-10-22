# potok

A local ledger in the terminal. One binary, one SQLite file, no server and no
account. Bank statements go in as PDFs; balances, debts and flows come out.

## Install

```sh
./install.sh
```

Builds a release binary and puts it in `~/.local/bin/potok` (`POTOK_BIN` picks
another folder). Needs a Rust toolchain. Run it again after pulling changes.

## Start

```sh
potok
```

opens the interface. The ledger lives in `~/.local/share/potok/ledger.db`
(`--db <path>` for another one). A ledger made by an older potok is upgraded
on open.

Screens — the digits switch them, `[` and `]` step through, `,` opens
Settings and `u` the journal of changes:

| | |
|---|---|
| 1 Home | the month: money in and out against last month, the months before as columns, where it went; beside it what is due in two weeks, what needs a look, debts. Everything on it is a list: `Tab` goes «where it went» → the month columns → the right side, and stepping across the columns puts that month into «where it went», which then says whose month it is showing. `Enter` opens the operations behind a category, a month, a regular payment, a debt — or does what the line says: reconciles that account, starts marking counterparties, takes the refunds back out of income |
| 2 Accounts | balances with each account's year as a line and when the bank last agreed; the last 30 days of the one under the cursor; `s` reconcile, `n` new operation |
| 3 Operations | the journal by day, each day with what it cost; `/` search, `a` one account, `d` dates, `f` fold by who, `o` order, `t` hide transfers, `Space` mark several |
| 4 Flows | where money went this period against the one before; `p` period |
| 5 Analytics | twelve months side by side, a month's categories, regular payments |
| 6 Budget | this month against each category's limit |
| 7 Debts | who owes whom and how it was paid; `n` new debt |
| 8 Counterparties | who is paid and who pays: `c` categorise, `C` all the uncategorised in turn, `x` rename, merge, a note of your own, add a spelling; `→` into the card, where the cursor stands on a spelling and `x` removes it; `i` imports a statement |
| 9 Categories | rename, merge, delete, set a monthly budget |
| 0 Duplicates | look-alike operations to remove or keep |
| `I` Statements | what was loaded and what it brought; `Enter` opens its operations, `x` rolls one back whole, `i` imports |
| `u` Changes | everything changed by hand; `x` undoes any of it |
| `,` Settings | language, theme, folders, base currency and rates, month start day, backups, screen lock |

Everywhere: `Enter` goes deeper, `Esc` back, `Tab` between list and details,
`x` lists what can be done with the thing under the cursor, `?` shows every
key, `L` locks the screen, `q` twice quits.

## Import

`i` on the Import screen lists PDFs from the statements folder (`~/Downloads`
unless set otherwise). Revolut and iCard statements are understood. Before
anything is written, the review window shows the new rows, the ones already in
the ledger, and what potok thinks of each: the other leg of a transfer, a
payment on a known debt, a date in the future. `Space` drops a row, `Enter`
rejects a hint, `w` writes. An import can be rolled back whole. Money back from a purchase («Reversal of …», «refund for …») is written as a refund: it takes the shop's spending down instead of counting as income; `potok refunds apply` does the same for rows written before.

iCard prints no running balance, so after its statement is written potok asks
what the bank shows for that account and reconciles at once (later: `s` on
Accounts).

## Backups

A copy of the ledger is made once a day by the first command that opens it,
into `backups` next to the ledger; thirty are kept. Point the folder at a
synced one to have the copy off the laptop:

```sh
potok set backup_dir "~/Library/Mobile Documents/com~apple~CloudDocs/potok"
```

The ledger, its copies and the archived statements are readable by you
only (0600; potok's own folder 0700, and the backup folder too when it holds
nothing but potok's copies) — set on every open, since a file made under the
usual umask is readable by every account on the Mac.

`potok backup` makes one now, `potok backups` lists them, `potok restore` puts
the latest back (the current ledger is backed up first). A restore is a
change like any other: it is in the journal, and with Google Drive set up it
goes there as the next copy.

A ledger opened with `--db` keeps its copies in `backups` beside it, whatever
the folder setting says — a copy to try something on must not put itself
among the real ledger's backups.

## Copy in Google Drive

The newest backup can also go to Google Drive, over the network, with
nothing installed: potok talks to Google's API with `curl`. Each copy is
encrypted on this Mac before it leaves (XChaCha20-Poly1305, key from your
passphrase by Argon2id), so Drive holds only ciphertext. potok asks for the
`drive.file` scope — it sees the files it made and nothing else there. The sign-in and the passphrase live outside the ledger: on macOS in the
Keychain, elsewhere in `~/.config/potok/secrets`, a file only you can read
(0600). Either way they are off the ledger, not away from your own programs —
anything already running as you can read the Keychain with `security
find-generic-password`, and the file with `cat`. `potok cloud` says which of
the two is in use.

**The passphrase is the only way back.** Write it down somewhere other than
this laptop; without it the copies cannot be opened by anyone, potok included.

**With gcloud on the Mac** — nothing to register. A project to count the
calls against, Drive switched on in it, and gcloud's sign-in for
applications with the narrow scope:

```sh
gcloud projects create potok-backup-<something>
gcloud services enable drive.googleapis.com --project potok-backup-<something>
gcloud auth application-default login --scopes=https://www.googleapis.com/auth/drive.file,https://www.googleapis.com/auth/cloud-platform
potok cloud login --project potok-backup-<something>
```

The project is kept in potok, not set on gcloud's sign-in, so other tools
that use that sign-in stay on their own projects.

**Without gcloud**, once, in [Google Cloud Console](https://console.cloud.google.com/), with the
Google account the copies should go to:

1. Create a project (any name, e.g. `potok`).
2. APIs & Services → Library → **Google Drive API** → Enable.
3. Google Auth Platform (OAuth consent screen): External; app name `potok`;
   your address as the support and developer contact. Under Audience add
   yourself as a test user, then **Publish app** — while it is in «Testing»,
   Google expires the sign-in after seven days.
4. Clients (Credentials) → Create → OAuth client ID → **Desktop app** → copy
   the Client ID and the Client secret.

Then:

```sh
potok cloud login
```

asks for the two, for the passphrase (twice), opens the browser to sign in,
and sends a first copy into a `potok-backups` folder. If Google says the app
is not verified, it is your own app: Advanced → Go to potok.

From then on a copy goes once a day in the background; thirty are kept.
`potok cloud` shows the last one sent and what is there, `potok cloud push`
sends now, `potok cloud get` downloads and decrypts the newest into the
backups folder for `potok restore <file>`. A failed send shows on Home and in
Settings.

## Another machine

On the device already set up:

```sh
potok cloud pair
```

reads out one line. On the new device:

```sh
potok cloud pair <that line>
potok cloud take
```

and the newest copy comes down. The code carries the OAuth client and the
sign-in — the two things the second device cannot work out for itself — sealed
with your passphrase, which it asks for and which is **not** inside the code.
So a code someone else sees opens nothing, and there is no client to register
and no browser to open on the new device. The sign-in is proved against Drive
before it is kept: a device that Google refuses says so now rather than
failing once a day in the background.

`potok cloud login` still works on the second device if you would rather type
the client in again; it needs **the same passphrase** either way.

**A device signed in through gcloud hands over a code too.** It has no client
of potok's own, so the code carries the client and the refresh token out of
gcloud's own credentials file, plus the project the calls count against —
Google refuses those credentials without one, and the new device has no gcloud
to ask. What the new device keeps is an ordinary potok sign-in: gcloud is never
installed there and never called. Measured before it was written: the same
credentials, stored as potok's own and sent with that project, list the real
folder in Drive.

On Linux the ledger and
the cloud work the same way; what differs is where the secrets live (above),
that the browser is opened with `xdg-open`, and that the device's name comes
from `hostname` rather than from the Mac's own. A statement is still imported
from a PDF, so a headless machine gets the file over `scp`.

## Several devices

One device writes at a time; any of them can. potok opens straight away and
asks Drive beside it — the answer lands in the status line a moment later, and
nothing in the ledger waits on the network:

* nobody writes — it puts up a mark «<this device> writes since 14:02», takes
  the newest copy if another device sent one, and opens as usual;
* another device writes — the screen turns **read only** the moment the answer
  comes, saying who and since when. `W` takes writing over (for a Mac that is
  off, or a potok that crashed there); a mark left for 12 hours is taken over
  without asking. The seconds before the answer are open for writing: an edit
  made in them stays, and goes to Drive beside the other device's copy as a
  conflict copy on the next start.

On leaving, the writer sends what changed and lets go of the mark — in a
process of its own, after the screen is gone, so quitting is instant. What it
did is on Home and in Settings (the last copy sent, and the error if it
failed); `potok cloud push` sends on demand. Each copy
has a generation, one more than the copy it started from, so a device that
comes back late never overwrites a newer copy: its ledger goes to Drive
beside it as `…-conflict-<device>.db.potok`, Home says so, and nothing is
lost silently. Changes are not merged — two ledgers that disagree are shown,
not guessed at.

The device's name is in Settings («This device»; `potok set device_name …`).
Without the interface: `potok cloud take` and `potok cloud release` do what
starting and leaving do. Only the default ledger takes part — one opened
with `--db` stays on this Mac.

On a second device: `potok cloud pair` on this one, `potok cloud pair <code>`
there, and start it — the newest copy comes down («Another machine» above).

## Settings

English by default, Russian a switch away (`,` → Language, or `potok set
language ru`). Six themes, and they differ in more than colour: **terminal** takes the
sixteen colours the terminal itself is set to (and so works on a light
background as well), square frames, doubled where the keys are; **midnight**
— deep blue ink, cornflower structure, mint and rose for money; **amber** — a
warm screen, amber text and orange structure; **mono** — greyscale and one
white accent, so the amounts are the only thing with a colour; **neon** —
ultraviolet, pink structure and turquoise income; **green** — a phosphor
screen that still keeps money red and green. All but the first
want truecolour and round their frames, thickening the one that has the keys.
Bars and lists grow into place over a third of a second when a screen opens —
long enough to see that the figures changed, short enough not to wait for. A month can start on a fixed day or on the
last working day of the month, when a salary comes; flows, analytics and
budgets all count from it, and any month's start can be corrected by hand
(`x` on it in Analytics) when a holiday moved the salary.

Totals and analytics are in a base currency. Rates come from the ECB (EUR
against the world) and the Bank of Russia (RUB) — `potok rates update`, or
daily on start if asked — or are typed by hand (`potok rate RUB 0.0098`);
a pair with no direct rate goes through EUR.

The screen lock asks a passphrase at start, on `L`, and after the idle
minutes set; a wrong passphrase makes the next try wait longer each time. It
is a screen lock, not protection for the file: the ledger is plain SQLite.

## For an agent

`potok mcp` is an MCP server over stdio, read-only by the database's own
refusal. For Claude Code:

```sh
claude mcp add potok -- potok mcp
```

## Putting things in the right category

A category lives on the counterparty, so marking one marks every operation of
theirs, past and future. One operation can have its own category as an
exception (`x` on it → «A category for this operation only»), and after that
potok asks whether to move the rest of that shop there too and remember it —
«yes» makes it the counterparty's category and takes its operations out of the
one they sat in. That is the way out of a migrated history where every row
carries its own «Other». Each step is a line in the journal (`u`) and can be
undone.

## Which potok is this

```sh
potok --version        # potok 0.2.0 (ab12cd3)
potok version          # that, plus the ledger it opens and where it was built from
potok update --check   # has the source moved on since this binary?
potok update           # build and install it again (--pull fetches the source first)
```

The hash is the commit it was built from; a `+` means it was built with
uncommitted changes, so that hash does not describe it exactly. There is
nothing to download: potok is built on the Mac it runs on, and `potok update`
runs `install.sh` in the folder the binary remembers. `CHANGELOG.md` says what
changed between versions.

## Commands

A note of your own goes on an operation (`x` on it, or `potok op <n> note
<text>`) and on a counterparty — the bank writes what it writes, the meaning
is yours.

`potok --help` lists them: import, add, debts, reconcile, duplicates, log,
undo, export and the rest. What the operation menu does is `potok op <number>
remove | restore | unpair | pair | category | split`; categories, budgets and
a month's start are `potok category …` and `potok month …`. A command that
only reads refuses a ledger that is not there, rather than creating an empty
one at a mistyped `--db`.

## License

MIT or Apache-2.0, whichever suits you — `LICENSE-MIT` and `LICENSE-APACHE`,
the pair the Rust ecosystem uses. Contributions are taken under the same two.
