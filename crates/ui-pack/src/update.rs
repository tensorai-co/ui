//! `ui-pack update-key` and `ui-pack update-feed`: signing keys and signed
//! feeds for `ui::update::check`, using the system `openssl`.

use std::path::{Path, PathBuf};
use std::process::Command;

fn openssl(args: &[&str], cwd: &Path) -> Result<Vec<u8>, String> {
    let out = Command::new("openssl")
        .args(args)
        .current_dir(cwd)
        .output()
        .map_err(|e| format!("run openssl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "openssl {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

/// Creates `update-private.pem` in `dir` (keep it secret, out of the repo) and
/// returns the base64 public key to pass to `ui::update::check`.
pub fn make_key(dir: &Path) -> Result<(PathBuf, String), String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let key = dir.join("update-private.pem");
    if key.exists() {
        return Err(format!(
            "{} already exists; not overwriting it",
            key.display()
        ));
    }
    openssl(
        &[
            "ecparam",
            "-name",
            "prime256v1",
            "-genkey",
            "-noout",
            "-out",
            "update-private.pem",
        ],
        dir,
    )?;
    Ok((key.clone(), public_key(&key)?))
}

/// The base64 uncompressed P-256 public key for a private key file.
pub fn public_key(private_key: &Path) -> Result<String, String> {
    let dir = private_key.parent().unwrap_or(Path::new("."));
    let name = private_key
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    let spki = openssl(
        &[
            "ec",
            "-in",
            name,
            "-pubout",
            "-outform",
            "DER",
            "-conv_form",
            "uncompressed",
        ],
        dir,
    )?;
    if spki.len() < 65 {
        return Err("openssl returned an unexpected public key".into());
    }
    Ok(base64(&spki[spki.len() - 65..]))
}

/// The signed feed JSON for `version` at `url`.
pub fn make_feed(
    private_key: &Path,
    version: &str,
    url: &str,
    notes: Option<&str>,
) -> Result<String, String> {
    let mut payload = format!(
        "{{\"version\":{},\"url\":{}",
        json_str(version),
        json_str(url)
    );
    if let Some(n) = notes {
        payload.push_str(&format!(",\"notes\":{}", json_str(n)));
    }
    payload.push('}');
    let tmp = std::env::temp_dir().join(format!("ui-pack-feed-{}.json", std::process::id()));
    std::fs::write(&tmp, &payload).map_err(|e| e.to_string())?;
    let key = std::fs::canonicalize(private_key)
        .map_err(|e| format!("{}: {e}", private_key.display()))?;
    let sig = openssl(
        &[
            "dgst",
            "-sha256",
            "-sign",
            &key.to_string_lossy(),
            &tmp.to_string_lossy(),
        ],
        Path::new("."),
    );
    let _ = std::fs::remove_file(&tmp);
    Ok(format!(
        "{{\"payload\":{},\"signature\":{}}}\n",
        json_str(&payload),
        json_str(&base64(&sig?))
    ))
}

fn json_str(s: &str) -> String {
    let mut out = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn base64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_and_json_strings() {
        assert_eq!(base64(b"ab"), "YWI=");
        assert_eq!(base64(b"abc"), "YWJj");
        assert_eq!(json_str("a\"b\n"), r#""a\"b\n""#);
    }
}
