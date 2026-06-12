//! Create flow + password staging: building the new-entry form, planning the
//! create LDIF, and staging passwords for both the create and edit paths.

use crate::config::EntryProfile;
use crate::ldap::worker::WorkerHandle;
use crate::schema::SchemaModel;
use crate::ui::edit_form::{build_edit_form, EditForm, FormMode};
use crate::workflows::create::{
    apply_static_defaults, empty_form_for_profile, fold_create_password, now_unix_secs_or_zero,
    plan_create, CreatePrep,
};
use crate::workflows::read_flow::ReadFlow;

use super::overlay::{Overlay, PendingAction};
use super::{allocate_number, App, Pane};

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
    // The cleartext password is staged into `pending_password` by the set-password
    // popup (the masked field is read-only inline), not carried in the form fields.
    let pending_password = form.pending_password.clone();
    // Defence in depth: never stage a cleartext password over a plain link.
    if pending_password.is_some() && !app.connection_encrypted {
        app.overlay = Some(Overlay::Error {
            text: "password change requires an encrypted connection".into(),
        });
        return;
    }
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
    match plan_create(read_flow.schema(), profile, &container, &edited) {
        CreatePrep::Confirm {
            dn,
            mut attrs,
            container,
            ldif,
        } => {
            // Fold the staged password (cleartext + optional Samba hashes) into the
            // new-entry Add, masking those values in the preview body. The widget's
            // `primary`/`samba` come from the resolved widgets for the new entry's
            // object classes; absent a password, keep the plain LDIF preview.
            let body = fold_create_password(
                &dn,
                &mut attrs,
                pending_password.as_deref(),
                &app.widgets,
                now_unix_secs_or_zero(),
            )
            .unwrap_or(ldif);
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
    widgets: &[crate::config::widget::ResolvedWidget],
    profile_idx: usize,
    container: String,
    samba_enabled: bool,
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
    // The create form has no objectClass field (filtered by empty_form_for_profile),
    // so use the profile's declared object classes for widget resolution.
    let ocs = &profile.object_classes;
    crate::ui::edit_form::tag_widget_fields(&mut form, widgets, ocs, false);
    // Inject resolver-driven kinds (Readonly / SambaSid / XOrdered) for fields
    // not yet bound by an explicit profile widget. Use profile OCs as above.
    let resolver =
        crate::config::resolver::WidgetResolver::new(schema, &[], widgets, samba_enabled);
    crate::ui::edit_form::inject_resolver_kinds(&mut form, &resolver, ocs);
    // Auto-inject ObjectClassPicker on the objectClass field.
    if let Some(f) = form
        .fields
        .iter_mut()
        .find(|f| f.label.eq_ignore_ascii_case("objectClass") && f.editable)
    {
        f.widget_binding = Some(crate::config::widget::WidgetKind::ObjectClassPicker);
    }
    // Auto-inject NextNumber on fields whose default is `{next:MIN-MAX}` so Enter
    // allocates the value at create time (otherwise it only resolves at save).
    crate::ui::edit_form::tag_next_number_fields(&mut form, &profile.defaults);
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
    let form = build_new_entry_form(
        read_flow.schema(),
        profile,
        &app.widgets,
        i,
        container,
        app.samba.is_some() && !app.read_only,
    );
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
            false,
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
