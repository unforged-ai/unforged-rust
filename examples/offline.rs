//! Check a signature on this machine with the offline binary:
//! `UNFORGED_LICENSE_FILE=nib.lic cargo run --example offline -- unforged-nib application.pdf card.png`
use unforged::nib::VerifyOptions;
use unforged::{Mode, Unforged};

#[tokio::main]
async fn main() -> unforged::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [binary, file, references @ ..] = args.as_slice() else {
        eprintln!("usage: offline <unforged-nib> <document> <reference>...");
        std::process::exit(2);
    };
    let client = Unforged::builder()
        .mode(Mode::Offline)
        .nib_binary(binary)
        .build()?;
    let licence = client.nib().licence().await?;
    println!(
        "licensed to {} until {}",
        licence.issued_to, licence.valid_until
    );
    let result = client
        .nib()
        .verify(file, VerifyOptions::references(references))
        .await?;
    println!("decision: {:?}", result.decision);
    for notice in &result.notices {
        println!("note: {notice}");
    }
    Ok(())
}
