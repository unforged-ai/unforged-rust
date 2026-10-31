//! The same code online and offline: `UNFORGED_MODE` picks which, and the references come from a
//! folder per person, `UNFORGED_REFERENCE_SOURCE=/secure/references/{subject}`.
use unforged::nib::{Decision, VerifyOptions};
use unforged::{Error, Unforged};

#[tokio::main]
async fn main() -> unforged::Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(file), Some(subject)) = (args.next(), args.next()) else {
        eprintln!("usage: same_code_both_modes <document> <subject>");
        std::process::exit(2);
    };
    let client = Unforged::from_env()?;
    match client
        .nib()
        .verify(file, VerifyOptions::subject(&subject))
        .await
    {
        Ok(r) => match r.decision {
            Some(Decision::AutoApprove | Decision::Approve) => println!("approved"),
            Some(Decision::Flag) => println!("needs someone to look"),
            other => println!("{other:?}"),
        },
        Err(e) if e.is_not_found() => println!("{subject} has no reference signatures yet"),
        Err(Error::CheckFailed { detail, .. }) => println!("couldn't check it: {detail}"),
        Err(e) => return Err(e),
    }
    Ok(())
}
