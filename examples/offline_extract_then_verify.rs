//! Cut every signature out of a signed contract, then check each one against its signer's
//! references. The crops can also become references for later checks.
//!
//! ```text
//! export UNFORGED_NIB_BINARY=/opt/unforged/unforged-nib UNFORGED_LICENSE_FILE=/opt/unforged/nib.lic
//! cargo run --example offline_extract_then_verify -- contract.pdf refs/
//! ```
use std::path::PathBuf;

use unforged::nib::{ExtractOptions, VerifyOptions};
use unforged::Unforged;

#[tokio::main]
async fn main() -> unforged::Result<()> {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [contract, refs] = args.as_slice() else {
        eprintln!("usage: offline_extract_then_verify <signed document> <references>");
        std::process::exit(2);
    };
    let client = Unforged::offline()?;
    let out = tempfile_dir()?;

    let found = client
        .nib()
        .extract_signatures([contract], &out, ExtractOptions::default())
        .await?;
    for doc in &found {
        println!(
            "{}: {} signature(s) on {} page(s)",
            doc.file,
            doc.signatures.len(),
            doc.pages
        );
        for s in &doc.signatures {
            let r = client
                .nib()
                .verify(&s.crop, VerifyOptions::references([refs]))
                .await?;
            println!("  page {} at {:?}: {:?}", s.page, s.bbox, r.decision);
        }
    }
    Ok(())
}

fn tempfile_dir() -> std::io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("unforged-extract-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}
