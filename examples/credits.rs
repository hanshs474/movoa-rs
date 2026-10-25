// MOVOA_API_KEY=sk-... cargo run --example credits
fn main() {
    match movoa::credits() {
        Ok(n) => println!("{n}"),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1)
        }
    }
}
