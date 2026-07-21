//! Neutral sambaSID computation from an edit form's `uidNumber`.
//!
//! UI-neutral (no terminal/tvision dependency): finds the sibling `uidNumber`
//! field value in the [`EditForm`] and delegates to [`crate::samba::sid`]. The
//! tvision ACTIVATE dispatch calls this when a `sambaSID`-bound field is
//! activated; `Ok(sid)` fills the field, `Err(msg)` opens an error overlay.

use crate::samba::SambaDomainInfo;
use crate::workflows::edit_form::EditForm;

/// Compute the `sambaSID` for `form` from its sibling `uidNumber` value and the
/// resolved Samba `domain`. Returns a user-facing error string when generation
/// is not possible (no domain SID configured, or `uidNumber` missing/empty/
/// non-numeric).
pub fn samba_sid_for_form(
    form: &EditForm,
    domain: Option<&SambaDomainInfo>,
) -> Result<String, String> {
    let uid = form
        .fields
        .iter()
        .find(|f| f.label.eq_ignore_ascii_case("uidNumber"))
        .and_then(|f| f.values.first().map(|s| s.as_str()));
    crate::samba::sid::generate_user_sid(domain, uid)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::FieldKind;
    use crate::workflows::edit_form::{EditField, EditForm, FormMode};
    use crate::workflows::form_model::WidgetSpec;

    fn field(label: &str, values: &[&str]) -> EditField {
        EditField {
            label: label.into(),
            must: false,
            editable: true,
            multi: false,
            secret: false,
            ordered: false,
            orphaned: false,
            kind: FieldKind::Text,
            widget: WidgetSpec::ReadOnlyText,
            widget_binding: None,
            values: values.iter().map(|s| s.to_string()).collect(),
            baseline: values.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn form_with(fields: &[(&str, &[&str])]) -> EditForm {
        EditForm {
            dn: "cn=test".into(),
            mode: FormMode::Edit,
            object_classes: vec![],
            fields: fields.iter().map(|(l, v)| field(l, v)).collect(),
            baseline_csn: None,
        }
    }

    fn domain() -> SambaDomainInfo {
        SambaDomainInfo {
            domain_sid: "S-1-5-21-1-2-3".into(),
            algorithmic_rid_base: 1000,
        }
    }

    #[test]
    fn computes_sid_from_sibling_uidnumber() {
        let form = form_with(&[("uidNumber", &["1000"]), ("sambaSID", &[])]);
        assert_eq!(
            samba_sid_for_form(&form, Some(&domain())).unwrap(),
            "S-1-5-21-1-2-3-3000"
        );
    }

    #[test]
    fn errors_without_uidnumber() {
        let form = form_with(&[("sambaSID", &[])]);
        assert!(samba_sid_for_form(&form, Some(&domain())).is_err());
    }

    #[test]
    fn errors_without_domain() {
        let form = form_with(&[("uidNumber", &["1000"]), ("sambaSID", &[])]);
        assert!(samba_sid_for_form(&form, None).is_err());
    }
}
