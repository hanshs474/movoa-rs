// MOVOA_API_KEY=sk-... cargo run --example generate -- "a paper lantern drifting over a night river"
fn main() {
    let prompt = std::env::args().nth(1).unwrap_or_else(|| "a paper lantern drifting over a night river, slow dolly".into());
    match movoa::generate_video(&prompt, &movoa::Options::new()) {
        Ok(url) => println!("{url}"),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1)
        }
    }
}
