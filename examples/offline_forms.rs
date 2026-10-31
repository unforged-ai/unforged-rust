//! Signed forms: give the client your form templates and name the box to check.
//!
//! ```text
//! export UNFORGED_NIB_BINARY=/opt/unforged/unforged-nib UNFORGED_LICENSE_FILE=/opt/unforged/nib.lic
//! cargo run --example offline_forms -- forms/loan-application.json application.pdf refs/
//! ```
//!
//! References can be earlier copies of the same form: the signature is taken from its box.
use std::path::PathBuf;

use unforged::nib::VerifyOptions;
use unforged::{Mode, Target, Unforged};

#[tokio::main]
async fn main() -> unforged::Result<()> {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [template, application, refs] = args.as_slice() else {
        eprintln!("usage: offline_forms <template.json> <signed form> <references>");
        std::process::exit(2);
    };
    let client = Unforged::builder()
        .mode(Mode::Offline)
        .template(template)
        .build()?;

    let applicant = client
        .nib()
        .verify(
            application,
            VerifyOptions::references([refs]).target(Target::named_box("applicant_signature")),
        )
        .await?;
    println!("applicant: {:?}", applicant.decision);

    let witness = client
        .nib()
        .verify(
            application,
            VerifyOptions::references([refs])
                .template_id("loan-application-v3")
                .target(Target::named_box("witness_signature")),
        )
        .await?;
    println!("witness: {:?}", witness.decision);
    Ok(())
}
