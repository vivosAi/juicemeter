//! Questions on the terminal, even when stdin is busy (e.g. piped JSON).

use anyhow::Context;
use std::io::{BufRead, Write};

/// Ask a yes/no question. Enter alone gives `default`.
pub fn confirm(question: &str, default: bool) -> anyhow::Result<bool> {
    let mut tty = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .context("no terminal to ask on; pass --yes, or --check to only look")?;
    write!(tty, "{question} {} ", if default { "[Y/n]" } else { "[y/N]" })?;
    let mut answer = String::new();
    std::io::BufReader::new(tty).read_line(&mut answer)?;
    Ok(match answer.trim() {
        "" => default,
        a => matches!(a, "y" | "Y" | "yes" | "Yes"),
    })
}
