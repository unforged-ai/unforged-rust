//! Check documents with Loupe, online or offline (`UNFORGED_MODE`):
//! `cargo run --example loupe -- statement.pdf payslip.pdf`
use unforged::loupe::LoupeOptions;
use unforged::Unforged;

#[tokio::main]
async fn main() -> unforged::Result<()> {
    let files: Vec<String> = std::env::args().skip(1).collect();
    let client = Unforged::from_env()?;
    let report = client.loupe().check(files, LoupeOptions::default()).await?;
    println!(
        "{:?} ({:.0}% likely altered)",
        report.result.verdict,
        report.result.probability * 100.0
    );
    for doc in &report.result.documents {
        println!("{}: {:?}", doc.name, doc.verdict);
        for s in &doc.signals {
            println!("  {}", s.reason);
        }
    }
    if let Some(id) = report.check_id {
        println!("check {id}");
    }
    Ok(())
}
