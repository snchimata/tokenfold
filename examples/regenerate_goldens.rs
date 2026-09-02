//! Regenerates every byte-exact golden output and its manifest SHA-256.
//!
//! Run only for an intentional contract change:
//! `cargo run -p tokenfold-core --example regenerate_goldens -- --write`.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
struct Manifest {
    golden: Vec<Golden>,
}

#[derive(Deserialize)]
struct Golden {
    transform_id: String,
    input: PathBuf,
    expected: PathBuf,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args().nth(1).as_deref() != Some("--write") {
        return Err("refusing to rewrite contracts without --write".into());
    }
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let manifest_path = root.join("tests/golden/MANIFEST.toml");
    let manifest_text = std::fs::read_to_string(&manifest_path)?;
    let manifest: Manifest = toml::from_str(&manifest_text)?;
    let mut hashes = Vec::with_capacity(manifest.golden.len());

    for golden in &manifest.golden {
        let input = std::fs::read(root.join(&golden.input))?;
        let output = match golden.transform_id.as_str() {
            "json_minify" => tokenfold_core::transforms::json::minify_json(&input)?,
            "log_compaction" => {
                tokenfold_core::transforms::logs::compact(std::str::from_utf8(&input)?, false)
                    .into_bytes()
            }
            "schema_compaction" => tokenfold_core::transforms::schema::compact_schema(&input, 1)?,
            "diff_compaction" => {
                tokenfold_core::transforms::diff::compact_diff(std::str::from_utf8(&input)?, true)
                    .into_bytes()
            }
            id => return Err(format!("unsupported golden transform {id:?}").into()),
        };
        std::fs::write(root.join(&golden.expected), &output)?;
        hashes.push(format!("{:x}", Sha256::digest(&output)));
    }

    let mut hashes = hashes.into_iter();
    let rewritten = manifest_text
        .lines()
        .map(|line| {
            if line.starts_with("sha256 = ") {
                let hash = hashes.next().ok_or("fewer hashes than manifest entries")?;
                Ok(format!("sha256 = \"{hash}\""))
            } else {
                Ok(line.to_string())
            }
        })
        .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?
        .join("\n");
    if hashes.next().is_some() {
        return Err("more hashes than manifest entries".into());
    }
    std::fs::write(manifest_path, format!("{rewritten}\n"))?;
    println!("regenerated {} golden fixtures", manifest.golden.len());
    Ok(())
}
