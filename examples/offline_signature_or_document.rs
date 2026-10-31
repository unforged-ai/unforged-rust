//! What you check can be a signature on its own or the whole signed document.
//!
//! ```text
//! export UNFORGED_NIB_BINARY=/opt/unforged/unforged-nib UNFORGED_LICENSE_FILE=/opt/unforged/nib.lic
//! cargo run --example offline_signature_or_document -- refs/ signature.png form.pdf photo.jpg
//! ```
use std::path::PathBuf;

use unforged::nib::{Verification, VerifyOptions};
use unforged::{Target, Unforged};

fn show(what: &str, r: &Verification) {
    println!(
        "{what}: {:?} ({} signature(s) found)",
        r.decision,
        r.signatures.len()
    );
    for s in &r.signatures {
        println!(
            "  page {} at {:?}: {:?} {:?}{}",
            s.page,
            s.bbox,
            s.decision,
            s.match_probability,
            if s.best_match { " (best match)" } else { "" }
        );
    }
}

#[tokio::main]
async fn main() -> unforged::Result<()> {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [refs, signature, document, photo] = args.as_slice() else {
        eprintln!("usage: offline_signature_or_document <references> <signature image> <document> <photo of a page>");
        std::process::exit(2);
    };
    let client = Unforged::offline()?;
    let nib = client.nib();

    let r = nib
        .verify(signature, VerifyOptions::references([refs]))
        .await?;
    show("a signature cut from a cheque or a pad", &r);

    let r = nib
        .verify(document, VerifyOptions::references([refs]))
        .await?;
    show("the whole document, every signature on it", &r);

    let r = nib
        .verify(
            document,
            VerifyOptions::references([refs]).target(Target::region(2, 140, 1980, 900, 260)),
        )
        .await?;
    show("only the box on page 2", &r);

    let crops = std::env::temp_dir().join("unforged-crops");
    let r = nib
        .verify(
            photo,
            VerifyOptions::references([refs])
                .document()
                .save_crops(&crops),
        )
        .await?;
    show("a phone photo of the signed page", &r);
    for s in &r.signatures {
        if let Some(crop) = &s.crop {
            println!("  saved {}", crop.display());
        }
    }
    Ok(())
}
