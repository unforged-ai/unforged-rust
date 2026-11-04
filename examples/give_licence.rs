//! Gives a client a short-lived licence and saves it for them. `UNFORGED_MODE` picks where it
//! comes from: your workspace online, or your licence server offline.
use unforged::licensing::{Include, NewLicence};
use unforged::Unforged;

#[tokio::main]
async fn main() -> unforged::Result<()> {
    let mut args = std::env::args().skip(1);
    let (Some(for_name), Some(out)) = (args.next(), args.next()) else {
        eprintln!("usage: give_licence <who it's for> <file to write> [days] [nib|loupe|both]");
        std::process::exit(2);
    };
    let mut new = NewLicence::new(&for_name);
    if let Some(days) = args.next() {
        new = new.days(days.parse().unwrap_or(30));
    }
    new = match args.next().as_deref() {
        Some("nib") => new.includes([Include::Nib]),
        Some("loupe") => new.includes([Include::Loupe]),
        _ => new,
    };
    let client = Unforged::from_env()?;
    let licensing = client.licensing();
    let licence = licensing.give(new).await?;
    std::fs::write(&out, format!("{}\n", licence.token.expose()))?;
    println!(
        "{} can use it from {} to {} ({}): {out}",
        licence.for_name,
        licence.valid_from,
        licence.valid_until,
        licence.includes.join(", ")
    );
    for l in licensing.licences().await? {
        println!(
            "{}  {} to {}  {}",
            l.status, l.valid_from, l.valid_until, l.for_name
        );
    }
    Ok(())
}
