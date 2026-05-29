//! RFC 2849 LDIF rendering for the write-path preview (spec §12 F1: "show
//! exactly what will be sent") and for ADD entries. Pure: no terminal, no
//! network. Golden-file tested.
//!
//! Scope notes (Decision D5):
//! * Values that are not RFC 2849 "safe" (leading space / `:` / `<`, control
//!   chars, non-ASCII) are base64-encoded with the `attr:: <b64>` form, using a
//!   hand-rolled encoder (no new dependency).
//! * Long-line wrapping (76 cols) is intentionally NOT applied — the output is a
//!   human preview, and unwrapped lines are easier to read. Re-add per RFC 2849
//!   if a real LDIF export is ever needed.
//! * Binary attributes are not rendered with real data here: M4's entry model
//!   carries binary attrs only as byte counts, so the preview shows a
//!   `# attr: <N bytes, not shown>` comment placeholder where applicable. The
//!   renderers in this module operate on string values only.

use std::collections::BTreeMap;

use crate::form::changeset::{ChangeSet, ModOp};

/// Render a [`ChangeSet`] as an LDIF `changetype: modrdn` and/or
/// `changetype: modify` record. A rename (when present) is emitted first as a
/// separate `modrdn` record, then — if there are attribute mods — a `modify`
/// record. Returns an empty string for an empty changeset.
pub fn render_changeset(cs: &ChangeSet) -> String {
    let mut out = String::new();

    if let Some(modrdn) = &cs.modrdn {
        out.push_str(&ldif_line("dn", &cs.dn));
        out.push_str("changetype: modrdn\n");
        out.push_str(&ldif_line("newrdn", &modrdn.new_rdn));
        out.push_str(&format!(
            "deleteoldrdn: {}\n",
            if modrdn.delete_old { 1 } else { 0 }
        ));
        if let Some(sup) = &modrdn.new_superior {
            out.push_str(&ldif_line("newsuperior", sup));
        }
    }

    if !cs.mods.is_empty() {
        if !out.is_empty() {
            out.push('\n');
        }
        out.push_str(&ldif_line("dn", &cs.dn));
        out.push_str("changetype: modify\n");
        for (i, op) in cs.mods.iter().enumerate() {
            if i > 0 {
                out.push_str("-\n");
            }
            render_mod_op(&mut out, op);
        }
        out.push_str("-\n");
    }

    out
}

/// Render a full ADD as an LDIF `changetype: add` record. Attribute order is the
/// `BTreeMap`'s (deterministic for tests / display).
pub fn render_add(dn: &str, attrs: &BTreeMap<String, Vec<String>>) -> String {
    let mut out = String::new();
    out.push_str(&ldif_line("dn", dn));
    out.push_str("changetype: add\n");
    for (attr, values) in attrs {
        for v in values {
            out.push_str(&ldif_line(attr, v));
        }
    }
    out
}

/// Append one `ModOp` stanza (the `add:`/`delete:`/`replace:` header plus value
/// lines) to `out`.
fn render_mod_op(out: &mut String, op: &ModOp) {
    match op {
        ModOp::Add { attr, values } => {
            out.push_str(&format!("add: {attr}\n"));
            for v in values {
                out.push_str(&ldif_line(attr, v));
            }
        }
        ModOp::Delete { attr, values } => {
            out.push_str(&format!("delete: {attr}\n"));
            for v in values {
                out.push_str(&ldif_line(attr, v));
            }
        }
        ModOp::Replace { attr, values } => {
            out.push_str(&format!("replace: {attr}\n"));
            for v in values {
                out.push_str(&ldif_line(attr, v));
            }
        }
    }
}

/// Render a single `attr: value` line, base64-encoding the value (`attr:: <b64>`)
/// when it is not an RFC 2849 SAFE-STRING. Always terminated by `\n`.
fn ldif_line(attr: &str, value: &str) -> String {
    if is_safe_value(value) {
        format!("{attr}: {value}\n")
    } else {
        format!("{attr}:: {}\n", b64(value.as_bytes()))
    }
}

/// RFC 2849 SAFE-STRING test for a UTF-8 string value:
/// * empty is safe;
/// * the first char must be SAFE-INIT-CHAR: not NUL/LF/CR, not a space, not `:`
///   or `<`, and ASCII (< 0x80);
/// * every other char must be SAFE-CHAR: not NUL/LF/CR and ASCII (< 0x80).
fn is_safe_value(v: &str) -> bool {
    let bytes = v.as_bytes();
    if bytes.is_empty() {
        return true;
    }
    let first = bytes[0];
    let safe_init = first != 0
        && first != b'\n'
        && first != b'\r'
        && first != b' '
        && first != b':'
        && first != b'<'
        && first < 0x80;
    if !safe_init {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|&b| b != 0 && b != b'\n' && b != b'\r' && b < 0x80)
}

/// Standard RFC 4648 base64 encoder (no line wrapping). Hand-rolled to avoid a
/// new dependency (Decision D5).
fn b64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = *chunk.get(1).unwrap_or(&0) as u32;
        let b2 = *chunk.get(2).unwrap_or(&0) as u32;
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(TABLE[((n >> 18) & 0x3f) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(TABLE[((n >> 6) & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(TABLE[(n & 0x3f) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::form::changeset::{ChangeSet, ModOp, ModRdn};

    fn attrs(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        let mut m = BTreeMap::new();
        for (k, vs) in pairs {
            m.insert(
                k.to_string(),
                vs.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
            );
        }
        m
    }

    #[test]
    fn b64_known_vectors() {
        // RFC 4648 test vectors.
        assert_eq!(b64(b""), "");
        assert_eq!(b64(b"f"), "Zg==");
        assert_eq!(b64(b"fo"), "Zm8=");
        assert_eq!(b64(b"foo"), "Zm9v");
        assert_eq!(b64(b"foob"), "Zm9vYg==");
        assert_eq!(b64(b"fooba"), "Zm9vYmE=");
        assert_eq!(b64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn is_safe_value_rules() {
        assert!(is_safe_value(""));
        assert!(is_safe_value("Alice"));
        assert!(is_safe_value("a@example.org"));
        assert!(!is_safe_value(" leading space"));
        assert!(!is_safe_value(":colon-first"));
        assert!(!is_safe_value("<lt-first"));
        assert!(!is_safe_value("naïve")); // non-ASCII
        assert!(!is_safe_value("line\nbreak"));
        // A colon NOT in the first position is fine.
        assert!(is_safe_value("a:b"));
    }

    #[test]
    fn golden_modify_simple() {
        let cs = ChangeSet {
            dn: "cn=Alice,ou=people,dc=example,dc=org".to_string(),
            modrdn: None,
            mods: vec![ModOp::Replace {
                attr: "sn".to_string(),
                values: vec!["Brown".to_string()],
            }],
        };
        assert_eq!(
            render_changeset(&cs),
            include_str!("testdata/ldif/modify_simple.ldif")
        );
    }

    #[test]
    fn golden_mixed_ops() {
        let cs = ChangeSet {
            dn: "uid=alice,ou=people,dc=example,dc=org".to_string(),
            modrdn: None,
            mods: vec![
                ModOp::Add {
                    attr: "mail".to_string(),
                    values: vec!["alice2@example.org".to_string()],
                },
                ModOp::Delete {
                    attr: "description".to_string(),
                    values: vec![],
                },
                ModOp::Replace {
                    attr: "sn".to_string(),
                    values: vec!["Brown".to_string()],
                },
            ],
        };
        assert_eq!(
            render_changeset(&cs),
            include_str!("testdata/ldif/modify_add_delete_replace.ldif")
        );
    }

    #[test]
    fn golden_modrdn() {
        let cs = ChangeSet {
            dn: "cn=Alice,ou=people,dc=example,dc=org".to_string(),
            modrdn: Some(ModRdn {
                new_rdn: "cn=Bob".to_string(),
                delete_old: true,
                new_superior: None,
            }),
            mods: vec![],
        };
        assert_eq!(
            render_changeset(&cs),
            include_str!("testdata/ldif/modrdn.ldif")
        );
    }

    #[test]
    fn golden_add() {
        let a = attrs(&[
            ("cn", &["Alice"]),
            ("objectClass", &["top", "inetOrgPerson"]),
            ("sn", &["Adams"]),
        ]);
        assert_eq!(
            render_add("cn=Alice,ou=people,dc=example,dc=org", &a),
            include_str!("testdata/ldif/add_entry.ldif")
        );
    }

    #[test]
    fn golden_base64() {
        // A value with a leading space and a UTF-8 value both base64-encode.
        let cs = ChangeSet {
            dn: "cn=Test,dc=example,dc=org".to_string(),
            modrdn: None,
            mods: vec![
                ModOp::Replace {
                    attr: "description".to_string(),
                    values: vec![" leading space".to_string()],
                },
                ModOp::Replace {
                    attr: "cn".to_string(),
                    values: vec!["naïve".to_string()],
                },
            ],
        };
        assert_eq!(
            render_changeset(&cs),
            include_str!("testdata/ldif/base64_value.ldif")
        );
    }
}
