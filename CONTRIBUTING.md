# Contributing

## The rules the code keeps

Read `SPEC.md` before changing anything that touches money. It is written
before the code, and where the two disagree the code is wrong until the
specification is corrected on purpose. The short version:

* **Money is `i64` minor units plus a currency.** Adding two amounts in
  different currencies does not compile. Rounding happens once, on the last
  step before the screen.
* **Balances and debt remainders are computed, never stored.** `potok verify`
  checks every invariant in §3 and names the row that breaks one.
* **A posted operation's amount, date and account never change.** A wrong row
  is removed — it stays visible and comes back with one command — and the
  right one is entered.
* **The interface runs no SQL and does no arithmetic on money.** `potok-core`
  hands it values; `cli/src/tui` draws them. A test holds the drawing to
  asking `theme.rs` for a colour rather than building its own.

## Before a pull request

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

Both run in CI on the same pinned toolchain. A change to a parser comes with
a sample taken from a real statement, not from memory of one: the first iCard
parser passed every test on a remembered format and silently lost 74 rows out
of 87.

New behaviour comes with the smallest test that fails without it. Try
breaking your own change and watch the test fail — that is the only proof the
test is holding anything.

## Commits

One change, one commit, a message that says what is now true and why — not
what files moved. The project's own history is the example.
