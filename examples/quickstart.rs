//! Check a signature through the API: `UNFORGED_API_KEY=unf_... cargo run --example quickstart -- application.pdf cust-88`
use unforged::nib::VerifyOptions;
use unforged::Unforged;

#[tokio::main]
async fn main() -> unforged::Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(file), Some(subject)) = (args.next(), args.next()) else {
        eprintln!("usage: quickstart <document> <subject>");
        std::process::exit(2);
    };
    let client = Unforged::from_env()?;
    let result = client
        .nib()
        .verify(file, VerifyOptions::subject(subject))
        .await?;
    println!("decision: {:?}", result.decision);
    if let Some(best) = result.best_match() {
        println!("match: {:?}", best.match_probability);
        for reason in &best.reasons {
            println!("  {reason}");
        }
    }
    Ok(())
}
