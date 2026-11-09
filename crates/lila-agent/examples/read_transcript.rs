//! Prints the items of a Claude Code transcript: `cargo run -p lila-agent --example read_transcript <file.jsonl>`.
fn main() {
    let path = std::env::args().nth(1).expect("transcript path");
    let mut t = lila_agent::claude::Transcript::new();
    for line in std::fs::read_to_string(path).expect("read").lines() {
        for item in t.feed(line) {
            println!("{}", serde_json::to_string(&item).unwrap().chars().take(160).collect::<String>());
        }
    }
}
