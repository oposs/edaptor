//! NT hash (`sambaNTPassword`) and `sambaPwdLastSet` — pure, golden-pinned.
//!
//! `sambaNTPassword = uppercase_hex(MD4(UTF-16LE(password)))` (spec §9).
//! `sambaPwdLastSet` is the Unix epoch in **seconds** as a decimal string; the
//! timestamp is injected so this stays unit-testable (the caller passes
//! `SystemTime::now()` converted to secs).

use md4::{Digest, Md4};

/// Compute the Samba NT hash: uppercase hex of MD4 over the UTF-16LE encoding
/// of the password. Always 32 uppercase-hex characters.
pub fn nt_hash(password: &str) -> String {
    let utf16le: Vec<u8> = password
        .encode_utf16()
        .flat_map(|u| u.to_le_bytes())
        .collect();
    let mut h = Md4::new();
    h.update(&utf16le);
    h.finalize().iter().map(|b| format!("{:02X}", b)).collect()
}

/// Render `sambaPwdLastSet`: the Unix epoch in seconds as a decimal string.
pub fn samba_pwd_last_set(now_unix_secs: u64) -> String {
    now_unix_secs.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nt_hash_empty_password_golden_vector() {
        assert_eq!(nt_hash(""), "31D6CFE0D16AE931B73C59D7E0C089C0");
    }

    #[test]
    fn nt_hash_password_golden_vector() {
        assert_eq!(nt_hash("password"), "8846F7EAEE8FB117AD06BDD830B7586C");
    }

    #[test]
    fn nt_hash_is_always_32_uppercase_hex_chars() {
        for pw in ["", "password", "S3cr3t!", "Ünïcödé", "a"] {
            let h = nt_hash(pw);
            assert_eq!(h.len(), 32, "hash for {pw:?} must be 32 chars");
            assert!(
                h.chars()
                    .all(|c| c.is_ascii_hexdigit() && !c.is_lowercase()),
                "hash for {pw:?} must be all uppercase hex: {h}"
            );
        }
    }

    #[test]
    fn samba_pwd_last_set_renders_decimal_seconds() {
        assert_eq!(samba_pwd_last_set(1_700_000_000), "1700000000");
    }
}
