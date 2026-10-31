# Working in this repo

Unforged for Rust (UNF-726): one client for the Unforged API and the offline binaries
(`unforged-nib`, `unforged-loupe`). The platform repo's `CLAUDE.md` holds the full house rules;
these are the ones that apply here.

## No ticket, no work

- Every change has a Linear ticket (team `UNF`, under the Nib epic UNF-6). No ticket yet: shape one
  and get it approved first.
- Branches are `<kind>/UNF-<n>-slug`; commit subjects and PR titles carry the ID (`UNF-123: ...`).
  Merge to main with `--no-ff`. Enforced by `scripts/ticket-ids` (pre-push, `just hooks`) and by
  ci.yml's `ticket-ids` job.
- Specs are Linear Documents on their ticket; plans are `.md` attachments. Code cites the ticket,
  never a file path or URL.

## The SDK's own rules

- The same call works online and offline wherever it can; a call one mode can't make returns
  `Error::NotSupported` saying what to do instead.
- Offline, the SDK never stores reference signatures (UNF-332), and the licence reaches a binary
  through its environment, never its command line.
- The binaries are run as they ship; never link the engine in.
- The README is customer-facing: what the customer does and sees, never our internals or vendors.

## Checks

`just check` runs what CI runs: fmt, clippy (all targets, all features, warnings as errors), the
tests, and `cargo deny`. SonarQube must be satisfied with no shortcuts.
