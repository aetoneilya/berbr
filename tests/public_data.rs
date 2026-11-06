use std::{fs, path::Path, process::Command};

#[test]
fn public_checkout_contains_only_synthetic_identifiers() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new("git")
        .current_dir(root)
        .args([
            "ls-files",
            "-z",
            "--cached",
            "--others",
            "--exclude-standard",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    // Keep the guard itself free of the literals it rejects.
    let forbidden = [
        ["/", "Users/"].concat(),
        ["i", ".udalov"].concat(),
        ["aetone", "ilya"].concat(),
        ["vk", ".team"].concat(),
        ["corp", ".mail.ru"].concat(),
        ["U", "CP-"].concat(),
    ];
    let mut hits = Vec::new();
    for file in output.stdout.split(|b| *b == 0).filter(|p| !p.is_empty()) {
        let file = std::str::from_utf8(file).unwrap();
        let Ok(content) = fs::read_to_string(root.join(file)) else {
            continue;
        };
        for (line, text) in content.lines().enumerate() {
            for pattern in &forbidden {
                if text.to_lowercase().contains(&pattern.to_lowercase()) {
                    hits.push(format!("{file}:{}: forbidden public identifier", line + 1));
                }
            }
        }
    }
    assert!(hits.is_empty(), "{}", hits.join("\n"));
}
