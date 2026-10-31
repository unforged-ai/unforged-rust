# Unforged for Rust

Check signatures with Nib and documents with Loupe from Rust, through our API or entirely on your
own servers with the offline binaries. The same code runs either way, so you can build against the
API and move to offline (or back) by changing one setting. Folio records, Sig signing packages and
your account's keys are here too, through the API.

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

### What Nib can compare

References can be in whatever form you hold them. A signature on its own (a crop from a cheque, a
signing pad or a scanned card, as PNG, JPEG, WebP, BMP or GIF) is used as it is. A whole page (a
signed PDF, a multi-page TIFF, an earlier copy of the same form) has its signature found for you:
the clearest one on the first pages, or the named box when the client has the form's template. If
a reference page holds two signatures that are equally likely, it's set aside rather than risk
learning the wrong one.

What you check can be a signature on its own or the whole document. On a document, every
signature is compared and the best match decides, unless you point at one with
`Target::region(page, x, y, w, h)` or `Target::named_box("applicant_signature")`. A phone photo or
scan of a single page is read as one signature unless you ask for it to be read as a page with
`VerifyOptions::document()`.

```rust
use unforged::{File, Target};

let refs = VerifyOptions::references(["refs/card.jpg", "refs/2019-mandate.pdf", "refs/pad.png"])
    .with_references([File::named("from-our-db.tif", tiff_bytes)]);
nib.verify("cheque-signature.png", refs.clone()).await?;
nib.verify("mandate.pdf", refs.clone().target(Target::region(2, 140, 1980, 900, 260))).await?;
nib.verify("photo-of-form.jpg", refs.document().save_crops("crops/")).await?;
```

The examples folder runs each of these against your own files:

| Example | Shows |
|---|---|
| `offline_reference_formats` | a folder of mixed references, each on its own, and the same from bytes |
| `offline_reference_sources` | checking by subject with a folder, your own lookup, or a map |
| `offline_signature_or_document` | a signature crop, a whole document, one region, a photo of a page |
| `offline_forms` | form templates and named boxes |
| `offline_extract_then_verify` | cutting out every signature, then checking each |

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

## Folio, Sig and your account (online)

```rust
use unforged::folio::{AccessReason, DownloadOptions, FilingOptions, Why};
use unforged::sig::{PackageOptions, Signer};

let folio = client.folio()?;
let receipt = folio
    .records()
    .file("agreement.pdf", FilingOptions::folder("loans/2026").retention_class("loan-7y"))
    .await?;
let pdf = folio
    .records()
    .download(&receipt.record_id, DownloadOptions::reason(AccessReason::code("audit")))
    .await?;
folio.records().extend_retention(&receipt.record_id, "2034-03-31", Why::code("dispute")).await?;

let package = client
    .sig()?
    .create_package(
        vec!["loan.pdf".into()],
        &[Signer::new("Ada Lovelace", "ada@example.com")],
        PackageOptions { title: Some("Your loan agreement".into()), ..Default::default() },
    )
    .await?;
```

A download is checked against the record's receipt before you get it, and a copy that doesn't
match comes back as `Error::Integrity` rather than being saved. Changes your organisation asks a
second person to approve come back as `Change::Pending` with the approval.

## Errors

Every call returns `unforged::Result`. The errors you're most likely to handle:

| Error | When |
|---|---|
| `Error::Api` with `kind` `NotFound` (`err.is_not_found()`) | No such subject, check or reference (online or offline) |
| `Error::CheckFailed` | The document couldn't be checked; `kind` and `detail` say why |
| `Error::Api` with `kind` `Authentication` or `PermissionDenied` | The key is wrong, or can't do this |
| `Error::Api` with `kind` `RateLimited` | Too many requests, or the month's checks are used up |
| `Error::Licence`, `Error::NotLicensed` | Offline: the licence is missing or expired, or doesn't cover this |
| `Error::CheckTimeout` | The check (or a queued filing) is still running; `job_id` finds it later |
| `Error::Integrity` | A downloaded record didn't match its receipt; try again |

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

## Licence

The SDK is under the MIT licence (see `LICENSE`). The offline binaries and their models come with
your Unforged licence, not this one.
