# Unforged for Rust

Check signatures with Nib and documents with Loupe from Rust, through our API or entirely on your
own servers with the offline binaries. The same code runs either way, so you can build against the
API and move to offline (or back) by changing one setting.

```toml
[dependencies]
unforged = { git = "https://github.com/unforged-ai/unforged-rust" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```

It needs Rust 1.89 or later. The client is async; turn on the `blocking` feature for code that
doesn't run an async runtime.

## Check a signature

```rust
use unforged::nib::VerifyOptions;
use unforged::Unforged;

#[tokio::main]
async fn main() -> unforged::Result<()> {
    let client = Unforged::new("unf_...")?;

    client.nib().enrol("cust-88", ["signature-card.png"], "signature_card").await?;

    let result = client
        .nib()
        .verify("application.pdf", VerifyOptions::subject("cust-88").reference("APP-1042"))
        .await?;
    println!("{:?}", result.decision); // Some(Approve), Some(Flag) …
    if let Some(best) = result.best_match() {
        println!("{:?}", best.match_probability); // Some(0.94)
        for reason in &best.reasons {
            println!("{reason}");
        }
    }
    Ok(())
}
```

`verify` waits for the answer. You can compare against reference files instead of an enrolled
person with `VerifyOptions::references(["card.png", "passport.pdf"])`, and point at one place on a
form with `.target(Target::named_box("applicant_signature"))` or
`.target(Target::region(1, x, y, w, h))`.

Files can be paths, bytes (`File::named("a.pdf", bytes)`), or URLs from your allowlist
(`File::url("https://…")`).

## Check a document with Loupe

```rust
use unforged::loupe::LoupeOptions;

let report = client.loupe().check(["statement.pdf", "payslip.pdf"], LoupeOptions::default()).await?;
println!("{:?} {:?}", report.result.verdict, report.result.top_reasons);
for doc in &report.result.documents {
    for signal in &doc.signals {
        println!("{}: {}", doc.name, signal.reason);
    }
}
```

Documents sent together are checked together, so a page that turns up twice, or details that don't
agree between them, are found too. Online, `client.loupe().checks()` gives you the review queue,
the evidence pack and the decision your team records.

## Run it offline

With the binaries and your licence from the delivery pack:

```rust
use unforged::{nib::VerifyOptions, Mode, Unforged};

// the licence comes from UNFORGED_LICENSE
let client = Unforged::builder()
    .mode(Mode::Offline)
    .nib_binary("/opt/unforged/unforged-nib") // or on your PATH
    .loupe_binary("/opt/unforged/unforged-loupe")
    .build()?;

let result = client
    .nib()
    .verify("application.pdf", VerifyOptions::references(["card.png", "passport.pdf"]))
    .await?;
```

The licence can live wherever you keep secrets: put the contents of `nib.lic` in the
`UNFORGED_LICENSE` environment variable, or pass `.licence(...)` with either the file's path or its
contents. It reaches the binary through its environment, never its command line.

Offline, the SDK never keeps reference signatures anywhere: you hand them over for each check, as
paths, bytes or a folder, from wherever you already hold them. Anything you pass as bytes is
written to a private temporary folder for the moment the check runs, then deleted; `.temp_dir(...)`
puts it somewhere of your choosing, such as an encrypted volume.

If your code checks against a person (`VerifyOptions::subject("cust-88")`), tell the client how to
find their references in your own systems, and the same call works online and offline:

```rust
use unforged::{File, ReferenceSource};

let source = ReferenceSource::function(|subject| vault_signature_cards(subject));
let client = Unforged::builder().mode(Mode::Offline).reference_source(source).build()?;
client.nib().verify("application.pdf", VerifyOptions::subject("cust-88")).await?;
```

A reference source can also be a map of subjects to files, or a folder with `{subject}` in its
path (`ReferenceSource::folder("/secure/references/{subject}")`), which the SDK only ever reads.
Adding or removing references (`enrol`, `subjects`) is online only.

```rust
let licence = client.nib().licence().await?;
println!("{} until {}", licence.issued_to, licence.valid_until);
```

To switch without touching code, set the environment and use `Unforged::from_env()`:

| Variable | Online | Offline |
|---|---|---|
| `UNFORGED_MODE` | `hosted` (the default) | `offline` |
| `UNFORGED_API_KEY` | your API key | |
| `UNFORGED_REGION` | optional: your region, as your API host names it | |
| `UNFORGED_NIB_BINARY` | | path to `unforged-nib` |
| `UNFORGED_LOUPE_BINARY` | | path to `unforged-loupe` |
| `UNFORGED_LICENSE` | | the licence itself, the text inside `nib.lic` |
| `UNFORGED_LICENSE_FILE` | | or the path to `nib.lic` |
| `UNFORGED_REFERENCE_SOURCE` | | optional: a folder with `{subject}` in its path |

A few things differ between the two. Online, a policy names one of your saved policies; offline
it's a policy JSON file or value. Offline reads local files only, so download anything at a URL
first. Usage figures, check history, confirmations, reviews and signer match are online only, and
finding every signature in a batch of documents (`extract_signatures`) is offline only. With
`LoupeOptions::default().lookups(true)`, an offline Loupe check asks the registers about the VAT,
company and firm numbers and web addresses it finds; only those are sent, never the document. A
call that isn't available returns `Error::NotSupported` with what to do instead.

## Batches and history (online)

```rust
use unforged::nib::{CheckItem, CreateOptions, ItemFilters};

let checks = client.nib().checks()?;
let job = checks
    .create(
        vec![
            CheckItem::new("app-1041.pdf", "cust-88").reference("APP-1041"),
            CheckItem::new("app-1042.pdf", "cust-91").reference("APP-1042"),
        ],
        CreateOptions { process: Some("loan-application".into()), wait: true, ..Default::default() },
    )
    .await?;
for r in &job.results {
    println!("{:?} {:?}", r.reference, r.decision);
}

let flagged = checks
    .items(&ItemFilters { decision: Some("FLAG".into()), ..Default::default() })
    .await?;
checks.confirm(&job.job_id, 0).await?; // a genuine signature, which may become a reference
println!("{}", client.nib().usage().await?.estimated_charge);
```

## Errors

Every call returns `unforged::Result`. The errors you're most likely to handle:

| Error | When |
|---|---|
| `Error::Api` with `kind` `NotFound` (`err.is_not_found()`) | No such subject, check or reference (online or offline) |
| `Error::CheckFailed` | The document couldn't be checked; `kind` and `detail` say why |
| `Error::Api` with `kind` `Authentication` or `PermissionDenied` | The key is wrong, or can't do this |
| `Error::Api` with `kind` `RateLimited` | Too many requests, or the month's checks are used up |
| `Error::Licence`, `Error::NotLicensed` | Offline: the licence is missing or expired, or doesn't cover this |
| `Error::CheckTimeout` | The check is still running; `job_id` finds it later |

Busy and rate-limited requests are tried again for you with the same idempotency key, so trying
again never checks a document twice.

## Webhooks

```rust
use unforged::webhooks;

let event = webhooks::construct_event(&raw_body, signature_header, &secret)?;
if event.event_type == "job.completed" {
    // …
}
```

Pass the body exactly as it arrived. We may send an event more than once, so skip an `event.id`
you've already handled.

## Development

```bash
just check   # fmt, clippy, tests and cargo deny, as CI runs them
just hooks   # the pre-push hook
```

The tests run against a stand-in API and stand-in binaries. To run them against the real offline
binaries too: `just test-real <pack folder> <licence file> <test kit files folder>`.
