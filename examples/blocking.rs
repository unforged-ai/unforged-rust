//! Without an async runtime: `cargo run --features blocking --example blocking -- application.pdf cust-88`
use unforged::blocking::Unforged;
use unforged::nib::VerifyOptions;

fn main() -> unforged::Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(file), Some(subject)) = (args.next(), args.next()) else {
        eprintln!("usage: blocking <document> <subject>");
        std::process::exit(2);
    };
    let client = Unforged::from_env()?;
    let result = client.nib().verify(file, VerifyOptions::subject(subject))?;
    println!("decision: {:?}", result.decision);
    Ok(())
}
