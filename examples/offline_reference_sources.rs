//! Check by subject offline: the SDK never stores references, so the client asks your own systems
//! for them each time.
//!
//! ```text
//! export UNFORGED_NIB_BINARY=/opt/unforged/unforged-nib UNFORGED_LICENSE_FILE=/opt/unforged/nib.lic
//! cargo run --example offline_reference_sources -- cust-88 cheque.pdf /secure/references
//! ```
//!
//! `/secure/references/cust-88/` holds that customer's references, in any format.
use std::collections::HashMap;
use std::path::PathBuf;

use unforged::nib::VerifyOptions;
use unforged::{File, Mode, ReferenceSource, Unforged};

fn client(source: ReferenceSource) -> unforged::Result<Unforged> {
    Unforged::builder()
        .mode(Mode::Offline)
        .reference_source(source)
        .build()
}

#[tokio::main]
async fn main() -> unforged::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [subject, questioned, root] = args.as_slice() else {
        eprintln!("usage: offline_reference_sources <subject> <questioned> <references root>");
        std::process::exit(2);
    };
    let root = PathBuf::from(root);

    let by_folder = client(ReferenceSource::folder(format!(
        "{}/{{subject}}",
        root.display()
    )))?;
    let r = by_folder
        .nib()
        .verify(questioned, VerifyOptions::subject(subject))
        .await?;
    println!("folder per customer: {:?}", r.decision);

    let store = root.clone();
    let by_function = client(ReferenceSource::function(move |subject| {
        let mut found = Vec::new();
        for entry in std::fs::read_dir(store.join(subject))? {
            let path = entry?.path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            found.push(File::named(name, std::fs::read(&path)?));
        }
        Ok(found)
    }))?;
    let r = by_function
        .nib()
        .verify(
            questioned,
            VerifyOptions::subject(subject).reference("CHQ-000231"),
        )
        .await?;
    println!("your own lookup, as bytes: {:?}", r.decision);

    let mut known = HashMap::new();
    known.insert(subject.clone(), vec![File::path(root.join(subject))]);
    let by_map = client(ReferenceSource::Map(known))?;
    let card = root.join(subject).join("card.png");
    let r = by_map
        .nib()
        .verify(
            questioned,
            VerifyOptions::subject(subject).with_references([card]),
        )
        .await?;
    println!(
        "a map, plus one more reference for this check: {:?}",
        r.decision
    );

    match by_map
        .nib()
        .verify(questioned, VerifyOptions::subject("nobody-we-know"))
        .await
    {
        Err(e) if e.is_not_found() => println!("no references for that subject: {e}"),
        other => println!("{other:?}"),
    }
    Ok(())
}
