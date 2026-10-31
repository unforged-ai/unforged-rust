//! References in whatever form you keep them, checked on this machine.
//!
//! ```text
//! export UNFORGED_NIB_BINARY=/opt/unforged/unforged-nib UNFORGED_LICENSE_FILE=/opt/unforged/nib.lic
//! cargo run --example offline_reference_formats -- cheque.png refs/
//! ```
//!
//! `refs/` can hold any mix of signature crops (PNG, JPEG, WebP, BMP, GIF), photos or scans of a
//! signature card, multi-page TIFFs and signed PDFs.
use std::path::PathBuf;

use unforged::nib::VerifyOptions;
use unforged::{File, Unforged};

#[tokio::main]
async fn main() -> unforged::Result<()> {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [questioned, refs] = args.as_slice() else {
        eprintln!("usage: offline_reference_formats <questioned> <folder of references>");
        std::process::exit(2);
    };
    let nib = Unforged::offline()?;
    let nib = nib.nib();

    let from_folder = nib
        .verify(questioned, VerifyOptions::references([refs]))
        .await?;
    println!(
        "against every file in {}: {:?}",
        refs.display(),
        from_folder.decision
    );

    let mut files: Vec<PathBuf> = std::fs::read_dir(refs)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file())
        .collect();
    files.sort();
    for reference in &files {
        let one = nib
            .verify(questioned, VerifyOptions::references([reference]))
            .await?;
        let p = one.best_match().and_then(|s| s.match_probability);
        println!(
            "  {} on its own: {:?} {:?}",
            reference.display(),
            one.decision,
            p
        );
    }

    let from_your_database: Vec<File> = files
        .iter()
        .map(|p| {
            let name = p.file_name().unwrap().to_string_lossy().into_owned();
            Ok(File::named(name, std::fs::read(p)?))
        })
        .collect::<std::io::Result<_>>()?;
    let from_bytes = nib
        .verify(
            File::bytes(std::fs::read(questioned)?),
            VerifyOptions::references(from_your_database),
        )
        .await?;
    println!(
        "the same, from bytes held in memory: {:?}",
        from_bytes.decision
    );
    for note in from_bytes.notes.iter().chain(&from_bytes.notices) {
        println!("note: {note}");
    }
    Ok(())
}
