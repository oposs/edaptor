//! Mapping LDAP result codes (RFC 4511 §A.1) to human-readable messages
//! (spec §10: "LDAP result codes map to human messages"). Pure, unit-tested; no
//! network. The worker calls [`result_code_message`] before a write result
//! crosses back to the UI, so the UI never sees a raw numeric code.

/// Map an LDAP result code + server diagnostic text to a human message.
///
/// Covers the codes edaptor's write path cares about; unknown codes fall back to
/// `"LDAP error <rc>: <text>"` (or without the trailing colon when `text` is
/// empty). Code `0` (success) is included for completeness, though success is
/// handled as `WriteOk` upstream rather than surfaced through this mapper.
///
/// The numeric code is always carried in the message. It is the one stable,
/// searchable identifier for a failure, and it tells the reader whether a bare
/// message like `"Constraint violation (LDAP 19)"` is all edaptor knows — some
/// servers send no diagnostic text at all (see [`crate::ldap::worker`]'s
/// transaction path).
pub fn result_code_message(rc: u32, text: &str) -> String {
    let base = match rc {
        0 => "Success",
        12 => "Server does not support a required control",
        16 => "No such attribute",
        19 => "Constraint violation",
        20 => "Attribute or value already exists",
        32 => "No such object",
        50 => "Insufficient access rights",
        64 => "Naming violation",
        65 => "Object class violation",
        66 => "Operation not allowed on non-leaf entry (it still has children)",
        68 => "Entry already exists",
        122 => "Entry was modified by someone else since you loaded it",
        _ => {
            if text.is_empty() {
                return format!("LDAP error {rc}");
            }
            return format!("LDAP error {rc}: {text}");
        }
    };
    if text.is_empty() {
        format!("{base} (LDAP {rc})")
    } else {
        format!("{base} (LDAP {rc}): {text}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_no_such_object() {
        assert_eq!(
            result_code_message(32, "no such object"),
            "No such object (LDAP 32): no such object"
        );
    }

    /// The numeric code survives even when the server sends no text — that is
    /// the case where the reader most needs something to look up.
    #[test]
    fn silent_server_still_yields_the_code() {
        assert_eq!(
            result_code_message(19, ""),
            "Constraint violation (LDAP 19)"
        );
    }

    #[test]
    fn maps_not_allowed_on_non_leaf() {
        let m = result_code_message(66, "subtree not empty");
        assert!(m.starts_with("Operation not allowed on non-leaf"), "m={m}");
        assert!(m.contains("subtree not empty"));
    }

    #[test]
    fn maps_insufficient_access() {
        assert_eq!(
            result_code_message(50, ""),
            "Insufficient access rights (LDAP 50)"
        );
    }

    #[test]
    fn maps_objectclass_violation() {
        let m = result_code_message(65, "missing required attribute sn");
        assert!(m.starts_with("Object class violation"), "m={m}");
        assert!(m.contains("sn"));
    }

    #[test]
    fn maps_entry_already_exists() {
        assert!(result_code_message(68, "").starts_with("Entry already exists"));
    }

    #[test]
    fn unknown_code_falls_back_to_text() {
        assert_eq!(
            result_code_message(123, "weird failure"),
            "LDAP error 123: weird failure"
        );
        assert_eq!(result_code_message(123, ""), "LDAP error 123");
    }

    #[test]
    fn maps_assertion_failed() {
        let m = result_code_message(122, "assertion failed");
        assert!(m.starts_with("Entry was modified by someone else"), "m={m}");
    }

    #[test]
    fn maps_unavailable_critical_extension() {
        let m = result_code_message(12, "");
        assert!(
            m.starts_with("Server does not support a required control"),
            "m={m}"
        );
    }
}
