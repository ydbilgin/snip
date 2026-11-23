//! snip never talks to the network: no uploads, no telemetry, no update check. This test reads the
//! sources and the dependency list and fails if anything network-shaped shows up.

use std::path::{Path, PathBuf};

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push(p);
        }
    }
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn sources_have_no_network_code() {
    let mut files = Vec::new();
    rust_files(&root().join("src"), &mut files);
    assert!(files.len() > 20, "found only {} source files", files.len());
    // Split so this file does not match itself.
    let banned = [
        ["std::", "net"].concat(),
        ["Tcp", "Stream"].concat(),
        ["Tcp", "Listener"].concat(),
        ["Udp", "Socket"].concat(),
        ["Win", "Http"].concat(),
        ["Internet", "Open"].concat(),
        ["URLDownload", "ToFile"].concat(),
        ["Win32_", "Networking"].concat(),
        ["Web", "Socket"].concat(),
        ["Http", "Client"].concat(),
        ["ws2", "_32"].concat(),
    ];
    let allowed_urls = ["http://www.w3.org/2000/svg"];
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        for b in &banned {
            assert!(!text.contains(b.as_str()), "{} contains {b}", f.display());
        }
        for scheme in [["http", "://"].concat(), ["https", "://"].concat()] {
            for (i, _) in text.match_indices(scheme.as_str()) {
                let rest = &text[i..];
                assert!(allowed_urls.iter().any(|u| rest.starts_with(u)), "{} has a URL: {}", f.display(), &rest[..rest.len().min(60)]);
            }
        }
    }
}

#[test]
fn dependencies_have_no_network_crates_or_features() {
    let toml = std::fs::read_to_string(root().join("Cargo.toml")).unwrap();
    assert!(!toml.contains(&["Win32_", "Networking"].concat()));
    assert!(!toml.contains("\"Web"));
    let lock = std::fs::read_to_string(root().join("Cargo.lock")).unwrap();
    let names: Vec<&str> = lock.lines().filter_map(|l| l.strip_prefix("name = \"")).map(|l| l.trim_end_matches('"')).collect();
    assert!(names.contains(&"snip"), "lock file parsed");
    let banned = ["reqwest", "hyper", "ureq", "curl", "isahc", "surf", "attohttpc", "tokio", "mio", "socket2", "native-tls", "rustls", "openssl"];
    for n in &names {
        assert!(!banned.contains(n), "network crate {n} in Cargo.lock");
    }
}
