//! Check a webhook's signature: `cargo run --example webhook -- body.json 't=...,v1=...' whsec_...`
use unforged::webhooks;

fn main() -> unforged::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [body, header, secret] = args.as_slice() else {
        eprintln!("usage: webhook <body file> <Unforged-Signature header> <secret>");
        std::process::exit(2);
    };
    let raw = std::fs::read(body)?;
    let event = webhooks::construct_event(&raw, Some(header), secret)?;
    println!("{} {}", event.event_type, event.id);
    Ok(())
}
