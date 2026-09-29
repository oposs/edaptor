//! `#[cfg(test)]` fixtures: a schema with the classes the rules know, plus an
//! argus-like and a demo-like sample (spec §5.1).

use std::collections::BTreeMap;

use crate::detect::model::{ContainerSample, Sample, SampleEntry};
use crate::ldap::worker::RawSubschema;
use crate::schema::SchemaModel;

const DSTR: &str = "1.3.6.1.4.1.1466.115.121.1.15";
const IA5: &str = "1.3.6.1.4.1.1466.115.121.1.26";
const INT: &str = "1.3.6.1.4.1.1466.115.121.1.27";

pub(crate) fn schema() -> SchemaModel {
    let at = |oid: &str, name: &str, syntax: &str, extra: &str| {
        format!("( {oid} NAME '{name}' SYNTAX {syntax}{extra} )")
    };
    let raw = RawSubschema {
        object_classes: vec![
            "( 2.5.6.0 NAME 'top' ABSTRACT MUST objectClass )".into(),
            "( 2.5.6.6 NAME 'person' SUP top STRUCTURAL MUST ( sn $ cn ) MAY ( userPassword $ description ) )".into(),
            "( 2.5.6.7 NAME 'organizationalPerson' SUP person STRUCTURAL MAY ou )".into(),
            "( 2.16.840.1.113730.3.2.2 NAME 'inetOrgPerson' SUP organizationalPerson STRUCTURAL MAY ( uid $ mail $ givenName $ displayName $ jpegPhoto $ entryCSN ) )".into(),
            "( 1.3.6.1.1.1.2.0 NAME 'posixAccount' SUP top AUXILIARY MUST ( cn $ uid $ uidNumber $ gidNumber $ homeDirectory ) MAY ( userPassword $ loginShell $ gecos $ description ) )".into(),
            "( 1.3.6.1.1.1.2.1 NAME 'shadowAccount' SUP top AUXILIARY MUST uid MAY shadowLastChange )".into(),
            "( 1.3.6.1.1.1.2.2 NAME 'posixGroup' SUP top STRUCTURAL MUST ( cn $ gidNumber ) MAY ( userPassword $ memberUid $ description ) )".into(),
            "( 1.3.6.1.4.1.7165.2.2.6 NAME 'sambaSamAccount' SUP top AUXILIARY MUST ( uid $ sambaSID ) MAY ( sambaAcctFlags $ sambaNTPassword ) )".into(),
            "( 2.5.6.9 NAME 'groupOfNames' SUP top STRUCTURAL MUST ( member $ cn ) MAY description )".into(),
            "( 2.5.6.5 NAME 'organizationalUnit' SUP top STRUCTURAL MUST ou MAY description )".into(),
            "( 1.3.6.1.4.1.7165.2.2.5 NAME 'sambaDomain' SUP top STRUCTURAL MUST ( sambaDomainName $ sambaSID ) )".into(),
        ],
        attribute_types: vec![
            at("2.5.4.0", "objectClass", "1.3.6.1.4.1.1466.115.121.1.38", ""),
            at("2.5.4.3", "cn", DSTR, ""),
            at("2.5.4.4", "sn", DSTR, ""),
            at("2.5.4.11", "ou", DSTR, ""),
            at("2.5.4.13", "description", DSTR, ""),
            at("0.9.2342.19200300.100.1.1", "uid", DSTR, ""),
            at("0.9.2342.19200300.100.1.3", "mail", IA5, ""),
            at("2.5.4.42", "givenName", DSTR, ""),
            at("2.16.840.1.113730.3.1.241", "displayName", DSTR, " SINGLE-VALUE"),
            at("0.9.2342.19200300.100.1.60", "jpegPhoto", "1.3.6.1.4.1.1466.115.121.1.28", ""),
            at("2.5.4.35", "userPassword", "1.3.6.1.4.1.1466.115.121.1.40", ""),
            at("1.3.6.1.1.1.1.0", "uidNumber", INT, " SINGLE-VALUE"),
            at("1.3.6.1.1.1.1.1", "gidNumber", INT, " SINGLE-VALUE"),
            at("1.3.6.1.1.1.1.2", "gecos", IA5, " SINGLE-VALUE"),
            at("1.3.6.1.1.1.1.3", "homeDirectory", IA5, " SINGLE-VALUE"),
            at("1.3.6.1.1.1.1.4", "loginShell", IA5, " SINGLE-VALUE"),
            at("1.3.6.1.1.1.1.12", "memberUid", IA5, ""),
            at("2.5.4.31", "member", "1.3.6.1.4.1.1466.115.121.1.12", ""),
            at("1.3.6.1.4.1.7165.2.1.20", "sambaSID", IA5, " SINGLE-VALUE"),
            at("1.3.6.1.4.1.7165.2.1.4", "sambaAcctFlags", IA5, " SINGLE-VALUE"),
            at("1.3.6.1.4.1.7165.2.1.25", "sambaNTPassword", IA5, " SINGLE-VALUE"),
            at("1.3.6.1.4.1.7165.2.1.47", "sambaDomainName", DSTR, " SINGLE-VALUE"),
            at("1.3.6.1.1.1.1.5", "shadowLastChange", INT, " SINGLE-VALUE"),
            at("1.3.6.1.4.1.4203.666.1.7", "entryCSN", DSTR, " SINGLE-VALUE NO-USER-MODIFICATION USAGE directoryOperation"),
        ],
        ldap_syntaxes: vec![],
    };
    SchemaModel::from_raw(&raw)
}

/// Build an entry from `(attr, values)` pairs.
pub(crate) fn e(dn: &str, pairs: &[(&str, &[&str])]) -> SampleEntry {
    let attrs: BTreeMap<String, Vec<String>> = pairs
        .iter()
        .map(|(k, vs)| (k.to_string(), vs.iter().map(|v| v.to_string()).collect()))
        .collect();
    SampleEntry {
        dn: dn.to_string(),
        attrs,
    }
}

/// A container sample whose presence map is derived from the entries' keys.
pub(crate) fn container(dn: &str, entries: Vec<SampleEntry>) -> ContainerSample {
    ContainerSample {
        dn: dn.to_string(),
        entries,
        present: BTreeMap::new(),
        partial: false,
    }
}

fn ou(name: &str, base: &str) -> SampleEntry {
    e(
        &format!("ou={name},{base}"),
        &[
            ("objectClass", &["top", "organizationalUnit"]),
            ("ou", &[name]),
        ],
    )
}

/// services-argus: `cn` RDN, `uid = cn`, gid = uid private groups, one shared
/// group (`staff`, 5020) sitting in the user block, shared groups at 8000+,
/// one user with a different shell (`u12`, /bin/tcsh).
pub(crate) fn argus_sample() -> Sample {
    let base = "dc=argus,dc=ch";
    let people = format!("ou=people,{base}");
    let groups = format!("ou=groups,{base}");
    let mut users = Vec::new();
    let mut group_entries = Vec::new();
    for i in 1..=12u64 {
        let name = format!("u{i:02}");
        let num = (5000 + i - 1).to_string();
        let shell = if i == 12 { "/bin/tcsh" } else { "/bin/bash" };
        let home = format!("/home/{name}");
        let given = format!("Given{i}");
        users.push(e(
            &format!("cn={name},{people}"),
            &[
                ("objectClass", &["top", "inetOrgPerson", "posixAccount"]),
                ("cn", &[name.as_str()]),
                ("uid", &[name.as_str()]),
                ("sn", &["Argus"]),
                ("givenName", &[given.as_str()]),
                ("uidNumber", &[num.as_str()]),
                ("gidNumber", &[num.as_str()]),
                ("homeDirectory", &[home.as_str()]),
                ("loginShell", &[shell]),
            ],
        ));
        group_entries.push(e(
            &format!("cn={name},{groups}"),
            &[
                ("objectClass", &["top", "posixGroup"]),
                ("cn", &[name.as_str()]),
                ("gidNumber", &[num.as_str()]),
                ("memberUid", &[name.as_str()]),
            ],
        ));
    }
    for (name, gid) in [
        ("staff", "5020"),
        ("dev", "8000"),
        ("ops", "8001"),
        ("web", "8002"),
    ] {
        group_entries.push(e(
            &format!("cn={name},{groups}"),
            &[
                ("objectClass", &["top", "posixGroup"]),
                ("cn", &[name]),
                ("gidNumber", &[gid]),
                ("memberUid", &["u01"]),
            ],
        ));
    }
    Sample {
        base_dn: base.to_string(),
        containers: vec![
            container(base, vec![ou("people", base), ou("groups", base)]),
            container(&people, users),
            container(&groups, group_entries),
        ],
        ..Default::default()
    }
}

/// The podman demo, reduced: users in two containers (`ou=people` with Samba,
/// shared primary group 100; `ou=users` with gid = uid but no private groups),
/// `groupOfNames` + `posixGroup` in `ou=groups`, a `sambaDomain` at the base.
pub(crate) fn demo_sample() -> Sample {
    let base = "dc=example,dc=org";
    let people = format!("ou=people,{base}");
    let users_c = format!("ou=users,{base}");
    let groups = format!("ou=groups,{base}");
    let mut people_entries = Vec::new();
    for i in 1..=5u64 {
        let uid = format!("p{i}");
        let given = format!("P{i}");
        let cn = format!("P{i} Person");
        let num = (10000 + i - 1).to_string();
        let home = format!("/home/{uid}");
        let mail = format!("{uid}@example.org");
        let sid = format!("S-1-5-21-1-2-3-{}", 3000 + i);
        people_entries.push(e(
            &format!("uid={uid},{people}"),
            &[
                (
                    "objectClass",
                    &[
                        "top",
                        "inetOrgPerson",
                        "posixAccount",
                        "shadowAccount",
                        "sambaSamAccount",
                    ],
                ),
                ("uid", &[uid.as_str()]),
                ("cn", &[cn.as_str()]),
                ("givenName", &[given.as_str()]),
                ("sn", &["Person"]),
                ("mail", &[mail.as_str()]),
                ("uidNumber", &[num.as_str()]),
                ("gidNumber", &["100"]),
                ("homeDirectory", &[home.as_str()]),
                ("loginShell", &["/bin/bash"]),
                ("sambaSID", &[sid.as_str()]),
            ],
        ));
    }
    let mut users_entries = Vec::new();
    for i in 1..=3u64 {
        let uid = format!("user0{i}");
        let num = (1000 + i - 1).to_string();
        let home = format!("/home/{uid}");
        users_entries.push(e(
            &format!("uid={uid},{users_c}"),
            &[
                (
                    "objectClass",
                    &["top", "inetOrgPerson", "posixAccount", "shadowAccount"],
                ),
                ("uid", &[uid.as_str()]),
                ("cn", &[uid.as_str()]),
                ("sn", &["User"]),
                ("uidNumber", &[num.as_str()]),
                ("gidNumber", &[num.as_str()]),
                ("homeDirectory", &[home.as_str()]),
            ],
        ));
    }
    let mut group_entries = Vec::new();
    for i in 1..=3u64 {
        let cn = format!("g{i}");
        let member = format!("uid=p{i},{people}");
        group_entries.push(e(
            &format!("cn={cn},{groups}"),
            &[
                ("objectClass", &["top", "groupOfNames"]),
                ("cn", &[cn.as_str()]),
                ("member", &[member.as_str()]),
            ],
        ));
        let pg = format!("pg{i}");
        let gid = (500 + i - 1).to_string();
        group_entries.push(e(
            &format!("cn={pg},{groups}"),
            &[
                ("objectClass", &["top", "posixGroup"]),
                ("cn", &[pg.as_str()]),
                ("gidNumber", &[gid.as_str()]),
                ("memberUid", &["p1"]),
            ],
        ));
    }
    let base_entries = vec![
        ou("people", base),
        ou("users", base),
        ou("groups", base),
        e(
            &format!("sambaDomainName=EXAMPLE,{base}"),
            &[
                ("objectClass", &["top", "sambaDomain"]),
                ("sambaDomainName", &["EXAMPLE"]),
                ("sambaSID", &["S-1-5-21-1-2-3"]),
            ],
        ),
    ];
    Sample {
        base_dn: base.to_string(),
        containers: vec![
            container(base, base_entries),
            container(&people, people_entries),
            container(&users_c, users_entries),
            container(&groups, group_entries),
        ],
        ..Default::default()
    }
}

/// Parse a TOML snippet of `[[profile]]` blocks into override blocks.
pub(crate) fn overrides(toml: &str) -> Vec<crate::config::ProfileOverride> {
    #[derive(serde::Deserialize)]
    struct W {
        #[serde(default)]
        profile: Vec<crate::config::ProfileOverride>,
    }
    toml::from_str::<W>(toml).expect("overrides parse").profile
}

#[test]
fn fixture_schema_parses_cleanly() {
    let s = schema();
    assert!(s.warnings.is_empty(), "{:?}", s.warnings);
    assert!(s.is_structural("posixGroup"));
    assert_eq!(
        s.structural_class(&["top".into(), "inetOrgPerson".into(), "posixAccount".into()]),
        Some("inetOrgPerson".into())
    );
}
