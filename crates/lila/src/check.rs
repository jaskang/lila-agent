//! Small helper shared by the git and GitHub modules: the end of a command's output.

/// Lines and characters of output kept: enough to see the failure, little enough to put in a message.
const TAIL_LINES: usize = 40;
const TAIL_CHARS: usize = 3000;

pub fn tail(out: &str) -> String {
    let lines: Vec<&str> = out.lines().collect();
    let kept = lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n");
    let n = kept.chars().count();
    if n <= TAIL_CHARS { kept } else { kept.chars().skip(n - TAIL_CHARS).collect() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_tail_is_the_end_of_the_output() {
        let long: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let t = tail(&long);
        assert!(t.starts_with("line 60") && t.ends_with("line 99"));
        assert_eq!(tail("short"), "short");
    }
}
