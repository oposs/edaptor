//! Create flow + password staging: building the new-entry form, planning the
//! create LDIF, and staging passwords for both the create and edit paths.

use crate::config::EntryProfile;
use crate::ldap::ldif::render_add;
use crate::ldap::worker::WorkerHandle;
use crate::schema::SchemaModel;
use crate::ui::edit_form::{build_edit_form, EditForm, FormMode};
use crate::workflows::create::{
    apply_static_defaults, empty_form_for_profile, mask_password_attrs, plan_create,
    stage_password, CreatePrep,
};
use crate::workflows::read_flow::ReadFlow;

use super::overlay::{Overlay, PendingAction};
use super::{allocate_number, object_classes_of, App, Pane};

pub(crate) fn prepare_create(
    app: &mut App,
    worker: &WorkerHandle,
    read_flow: &mut ReadFlow,
    profiles: &[EntryProfile],
    base_dn: &str,
) {
    let Some(form) = app.form.as_ref() else {
        return;
    };
    let (profile_idx, container) = match &form.mode {
        FormMode::Create {
            profile_idx,
            container,
        } => (*profile_idx, container.clone()),
        FormMode::Edit => return,
    };
    let Some(profile) = profiles.get(profile_idx) else {
        return;
    };
    let mut edited = form.to_edit_entry();
    // Fill empty fields from the profile's defaults; autonumber fields need a
    // synchronous directory scan (which may refuse on a truncated result).
    let autonum = apply_static_defaults(&profile.defaults, &mut edited.attrs);
    for (attr, min, max) in autonum {
        match allocate_number(worker, base_dn, &attr, min, max) {
            Ok(n) => {
                edited.attrs.insert(attr, vec![n.to_string()]);
            }
            Err(text) => {
                app.overlay = Some(Overlay::Error { text });
                return;
            }
        }
    }
    // Strip the password + confirm pseudo-fields (validating they match) BEFORE
    // building/validating the entry; the cleartext is injected into the real Add
    // afterwards and masked in the preview.
    let password = match &profile.password {
        Some(spec) => match stage_password(spec, &mut edited.attrs) {
            Ok(pw) => pw,
            Err(text) => {
                app.overlay = Some(Overlay::Error { text });
                return;
            }
        },
        None => None,
    };
    match plan_create(read_flow.schema(), profile, &container, &edited) {
        CreatePrep::Confirm {
            dn,
            mut attrs,
            container,
            ldif,
        } => {
            // Inject the password (cleartext + optional Samba hashes) into the real
            // Add, and mask those values in the preview body.
            let body = match (&profile.password, &password) {
                (Some(spec), Some(cleartext)) => {
                    let samba = spec.samba
                        && attrs
                            .get("objectClass")
                            .map(|ocs| {
                                ocs.iter()
                                    .any(|o| o.eq_ignore_ascii_case("sambaSamAccount"))
                            })
                            .unwrap_or(false);
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    for (k, v) in crate::samba::password::password_add_attrs(
                        cleartext,
                        &spec.ldap_attribute,
                        samba,
                        now,
                    ) {
                        attrs.insert(k, v);
                    }
                    render_add(&dn, &mask_password_attrs(&attrs, &spec.ldap_attribute))
                }
                _ => ldif,
            };
            app.overlay = Some(Overlay::Confirm {
                title: "Create this entry?".to_string(),
                body,
                action: PendingAction::Create {
                    dn,
                    attrs,
                    parent: container,
                },
            });
        }
        CreatePrep::Error(text) => {
            app.overlay = Some(Overlay::Error { text });
        }
    }
}

/// Build an empty Create-mode pane-3 form for `profile` (index `profile_idx`),
/// to be added under `container`. Editable fields are forced single-value so the
/// mandatory attributes can be typed inline (a second value is added post-create
/// via the value-editor popup). No relations are attached on create (parity with
/// the previous modal create path).
pub(crate) fn build_new_entry_form(
    schema: &SchemaModel,
    profile: &EntryProfile,
    pickers: &[crate::config::relation::ResolvedPicker],
    profile_idx: usize,
    container: String,
) -> EditForm {
    let model = empty_form_for_profile(schema, profile);
    let mut form = build_edit_form(&model, schema, false);
    for field in &mut form.fields {
        if field.editable {
            field.multi = false;
        }
    }
    form.mode = FormMode::Create {
        profile_idx,
        container,
    };
    // When the profile declares a password, replace the schema password field
    // with the masked password + confirm fields.
    if let Some(spec) = &profile.password {
        crate::ui::edit_form::inject_password_fields(&mut form, spec);
    }
    // Tag picker-bound fields so Enter opens the unified picker overlay.
    let ocs = object_classes_of(&form);
    crate::ui::edit_form::tag_picker_fields(&mut form, pickers, &ocs, false);
    // Final step: order fields after injection/tagging set secret/picker flags.
    crate::ui::edit_form::order_fields(&mut form);
    form
}

/// Install a fresh Create-mode form for `profiles[i]` into pane 3 and focus it.
/// The container is the profile's `search_base` (or `base_dn` when empty).
pub(crate) fn open_create_form(
    app: &mut App,
    read_flow: &mut ReadFlow,
    profiles: &[EntryProfile],
    i: usize,
    base_dn: &str,
) {
    let Some(profile) = profiles.get(i) else {
        return;
    };
    let container = if profile.search_base.is_empty() {
        base_dn.to_string()
    } else {
        profile.search_base.clone()
    };
    let form = build_new_entry_form(read_flow.schema(), profile, &app.pickers, i, container);
    app.form = Some(form);
    app.form_focus = 0;
    app.form_scroll = 0;
    app.overlay = None;
    // Focus the form pane so keystrokes edit the new entry's fields.
    app.focus = Pane::Form;
    app.status = format!(
        "New {} — fill fields, Alt+S to create, Esc to cancel.",
        profile.name
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::app::test_support::*;
    use crate::ui::edit_form::FormMode;

    #[test]
    fn build_new_entry_form_is_create_mode_and_single_value() {
        let form = build_new_entry_form(
            &user_schema(),
            &create_user_profile(),
            &[],
            0,
            "ou=people,dc=example,dc=org".to_string(),
        );
        assert!(form.is_new());
        match &form.mode {
            FormMode::Create {
                profile_idx,
                container,
            } => {
                assert_eq!(*profile_idx, 0);
                assert_eq!(container, "ou=people,dc=example,dc=org");
            }
            _ => panic!("expected Create mode"),
        }
        // every editable field is forced single-value for inline create
        assert!(form.fields.iter().all(|f| !(f.editable && f.multi)));
    }
}
