//! Headless smoke tests for the M3 read flow (no tty, no network).
//!
//! These exercise the pure logic below the facade: menu assembly, the
//! schema-driven form model, and id-correlated response handling. The tty-bound
//! pieces (`Shell`, `build_outline`, `build_entry_dialog`, `confirm_error`,
//! `show_entry_dialog`) require a terminal and are NOT covered here.

use std::collections::BTreeMap;

use edaptor::app::{build_menu_defs, CM_QUIT};
use edaptor::config::EntryProfile;
use edaptor::ldap::worker::{LdapEntry, RawSubschema, Response};
use edaptor::schema::SchemaModel;
use edaptor::ui::form::{build_form_model, WidgetSpec};
use edaptor::workflows::read_flow::{ReadFlow, ReadOutcome};

fn profile(name: &str, object_class: &str) -> EntryProfile {
    EntryProfile {
        name: name.to_string(),
        object_class: object_class.to_string(),
        ..Default::default()
    }
}

#[test]
fn menu_defs_smoke() {
    let profiles = vec![
        profile("Users", "inetOrgPerson"),
        profile("Groups", "groupOfNames"),
    ];
    let defs = build_menu_defs(&profiles);
    let labels: Vec<&str> = defs.iter().map(|d| d.label.as_str()).collect();
    assert_eq!(labels, vec!["Users", "Groups", "Browser", "Quit"]);
    assert_eq!(defs.last().unwrap().command, CM_QUIT);
}

#[test]
fn form_model_smoke() {
    let raw = RawSubschema {
        object_classes: vec![
            "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
            "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) MAY description )"
                .to_string(),
            "( 1.2.3 NAME 'demoPerson' SUP person STRUCTURAL MAY ( active $ manager ) )"
                .to_string(),
        ],
        attribute_types: vec![
            "( 2.5.4.3 NAME 'cn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
            "( 2.5.4.4 NAME 'sn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string(),
            "( 1.1.3 NAME 'active' SYNTAX 1.3.6.1.4.1.1466.115.121.1.7 )".to_string(),
            "( 1.1.4 NAME 'manager' SYNTAX 1.3.6.1.4.1.1466.115.121.1.12 )".to_string(),
        ],
        ldap_syntaxes: vec![],
    };
    let schema = SchemaModel::from_raw(&raw);

    let mut attrs = BTreeMap::new();
    attrs.insert("cn".to_string(), vec!["Alice".to_string()]);
    attrs.insert("sn".to_string(), vec!["Adams".to_string()]);
    attrs.insert("active".to_string(), vec!["TRUE".to_string()]);
    attrs.insert(
        "manager".to_string(),
        vec!["cn=boss,dc=example,dc=org".to_string()],
    );
    let entry = LdapEntry {
        dn: "cn=Alice,dc=example,dc=org".to_string(),
        attrs,
        bin_attrs: BTreeMap::new(),
    };

    let model = build_form_model(&schema, &["demoPerson"], &entry, &[]);
    assert!(!model.fields.is_empty());
    let widget = |name: &str| {
        model
            .fields
            .iter()
            .find(|f| f.label == name)
            .unwrap()
            .widget
            .clone()
    };
    assert_eq!(widget("cn"), WidgetSpec::ReadOnlyText);
    assert_eq!(widget("active"), WidgetSpec::DisabledCheckBox(true));
    assert_eq!(widget("manager"), WidgetSpec::ReadOnlyDn);
    assert!(
        model.fields.iter().any(|f| f.label == "cn" && f.is_must),
        "cn must be flagged MUST"
    );
}

#[test]
fn correlation_smoke() {
    // The shared id-keyed correlation: a matching id produces a form, a foreign
    // id is ignored. (The browser's two-interleaved-ids property is covered in
    // its own unit test; here we exercise the read flow's mechanism.)
    let raw = RawSubschema {
        object_classes: vec![
            "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".to_string(),
            "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST cn )".to_string(),
        ],
        attribute_types: vec![
            "( 2.5.4.3 NAME 'cn' SYNTAX 1.3.6.1.4.1.1466.115.121.1.15 )".to_string()
        ],
        ldap_syntaxes: vec![],
    };
    let flow = ReadFlow::new(SchemaModel::from_raw(&raw));

    let mk = |dn: &str, cn: &str| {
        let mut attrs = BTreeMap::new();
        attrs.insert("objectClass".to_string(), vec!["person".to_string()]);
        attrs.insert("cn".to_string(), vec![cn.to_string()]);
        LdapEntry {
            dn: dn.to_string(),
            attrs,
            bin_attrs: BTreeMap::new(),
        }
    };

    let form_a = flow.form_for(&mk("cn=a,dc=example,dc=org", "a"), &[]);
    let form_b = flow.form_for(&mk("cn=b,dc=example,dc=org", "b"), &[]);
    assert_eq!(form_a.title, "cn=a,dc=example,dc=org");
    assert_eq!(form_b.title, "cn=b,dc=example,dc=org");

    // A response whose id was never registered must be rejected.
    let mut flow2 = ReadFlow::new(SchemaModel::from_raw(&RawSubschema::default()));
    let resp = Response::Entries {
        id: 12345,
        entries: vec![mk("cn=a,dc=example,dc=org", "a")],
    };
    assert!(matches!(flow2.on_response(&resp), ReadOutcome::Ignored));
}
