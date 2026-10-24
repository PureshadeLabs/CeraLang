//! Prints the encoding of each line of stdin (for comparing against GNU as).
use std::io::BufRead;
fn main() {
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        match cera_syntax::asm::assemble_raw(&line) {
            Ok(b) => println!("{}", b.iter().map(|x| format!("{x:02x}")).collect::<Vec<_>>().join("")),
            Err(e) => println!("ERR {e}"),
        }
    }
}
