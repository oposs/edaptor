//! Live test (gated by EDAPTOR_TEST_LDAP_URI): profile detection against the
//! podman demo server. Start it with `scripts/test-ldap.sh start`.
//! Every test that calls `load` or `sample` goes through the real worker
//! sampling path (`run_sample_search`), not a fake searcher.

use edaptor::config::Config;
use edaptor::detect::load::{load_profiles, LoadedProfiles, ProfileInputs};
use edaptor::detect::sample::{sample, Budget, WorkerSearcher};
use edaptor::ldap::worker::{Request, Response, SampleParams, SearchScope, WorkerHandle};

fn conn_only(uri: &str) -> Config {
    toml::from_str(&format!(
        "[server]\nuri = \"{uri}\"\nbase_dn = \"dc=example,dc=org\"\n[auth]\nbind_dn = \"cn=admin,dc=example,dc=org\"\npassword_source = \"env:EDAPTOR_TEST_ADMIN_PW\"\n"
    ))
    .unwrap()
}
fn demo(uri: &str) -> Config {
    let text = include_str!("../examples/demo-config.toml").replace("ldap://localhost:11389", uri);
    toml::from_str(&text).unwrap()
}
fn pw() -> String {
    std::env::var("EDAPTOR_TEST_ADMIN_PW").unwrap_or_else(|_| "adminpassword".into())
}
macro_rules! live {
    () => {
        match std::env::var("EDAPTOR_TEST_LDAP_URI") {
            Ok(u) => u,
            Err(_) => {
                eprintln!("SKIP: EDAPTOR_TEST_LDAP_URI not set");
                return;
            }
        }
    };
}
fn load(cfg: Config) -> LoadedProfiles {
    let inputs = ProfileInputs::from_config(&cfg);
    let worker = WorkerHandle::spawn(cfg, pw()).expect("bind");
    load_profiles(&worker, &inputs).expect("load")
}

#[test]
fn detected_only_yields_the_expected_profiles() {
    let uri = live!();
    let r = edaptor::run_profiles(conn_only(&uri), pw(), true).expect("profiles");
    let t: toml::Table = toml::from_str(&r.toml).expect("valid TOML");
    let names: Vec<String> = t["profile"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap().to_string())
        .collect();
    for want in [
        "user-people",
        "user-users",
        "posixgroup-groups",
        "group-groups",
    ] {
        assert!(
            names.iter().any(|n| n == want),
            "{want} missing from {names:?}"
        );
    }
}

#[test]
fn live_sampling_detects_the_demo_users() {
    let uri = live!();
    let l = load(conn_only(&uri));
    assert!(l.detection_error.is_none(), "{:?}", l.detection_error);
    assert!(
        l.containers_sampled >= 4,
        "sampled {}",
        l.containers_sampled
    );
    let people = l
        .detected
        .iter()
        .find(|d| d.name == "user-people")
        .expect("user-people detected from the live sample");
    assert!(people
        .container
        .eq_ignore_ascii_case("ou=people,dc=example,dc=org"));
    assert_eq!(people.rdn_attr.value, "uid");
    assert_eq!(people.sampled, edaptor::detect::SAMPLE_SIZE as usize);
    assert!(people.partial, "600 users, 200 sampled");
    assert!(people
        .object_classes
        .value
        .iter()
        .any(|c| c.eq_ignore_ascii_case("posixAccount")));
}

#[test]
fn sampling_never_reads_password_values() {
    let uri = live!();
    assert!(
        !edaptor::detect::SAMPLE_ATTRS
            .iter()
            .any(|a| a.to_lowercase().contains("password")),
        "SAMPLE_ATTRS must not request secrets"
    );
    let cfg = conn_only(&uri);
    let worker = WorkerHandle::spawn(cfg, pw()).unwrap();
    let s = sample(
        &mut WorkerSearcher(&worker),
        "dc=example,dc=org",
        &Budget::new(edaptor::detect::DETECT_BUDGET),
    )
    .unwrap();
    let people = s
        .containers
        .iter()
        .find(|c| c.dn.eq_ignore_ascii_case("ou=people,dc=example,dc=org"))
        .expect("people container sampled");
    assert!(!people.entries.is_empty());
    // The server does hold userPassword: the types-only pass sees the name ...
    assert!(
        people
            .present
            .values()
            .any(|set| set.iter().any(|a| a.eq_ignore_ascii_case("userPassword"))),
        "types-only presence must report userPassword for the demo users"
    );
    // ... but no sampled entry carries a value for it.
    for c in &s.containers {
        for e in &c.entries {
            assert!(
                !e.attrs
                    .keys()
                    .any(|k| k.to_lowercase().contains("password")),
                "{} carries a secret attribute: {:?}",
                e.dn,
                e.attrs.keys().collect::<Vec<_>>()
            );
        }
    }
}

#[test]
fn merged_order_and_exact_scope() {
    let uri = live!();
    let l = load(conn_only(&uri));
    let pos = |n: &str| {
        l.profiles
            .iter()
            .position(|p| p.name == n)
            .unwrap_or_else(|| panic!("{n}"))
    };
    assert!(pos("user-people") < pos("user-users"));
    let here: Vec<&str> = edaptor::workflows::create::profiles_for_container(
        &l.profiles,
        "ou=people,dc=example,dc=org",
    )
    .into_iter()
    .map(|i| l.profiles[i].name.as_str())
    .collect();
    assert!(here.contains(&"user-people"), "{here:?}");
    assert!(
        !here
            .iter()
            .any(|n| n.starts_with("organizationalunit-") || n.starts_with("sambadomain-")),
        "{here:?}"
    );
}

#[test]
fn demo_config_produces_no_duplicates() {
    let uri = live!();
    let l = load(demo(&uri));
    let mut seen = std::collections::HashSet::new();
    for p in &l.profiles {
        assert!(
            seen.insert(p.name.to_lowercase()),
            "duplicate name {}",
            p.name
        );
    }
    let mut keys = std::collections::HashSet::new();
    for p in &l.profiles {
        let st = l
            .schema
            .structural_class(&p.object_classes)
            .unwrap_or_default()
            .to_lowercase();
        assert!(
            keys.insert((edaptor::detect::normalize_dn(&p.search_base), st.clone())),
            "two profiles for {} / {st}",
            p.search_base
        );
    }
    assert!(l.profiles.iter().any(|p| p.name == "user"));
    assert!(!l.profiles.iter().any(|p| p.name == "user-people"));
}

#[test]
fn passwd_resolves_with_a_connection_only_config() {
    let uri = live!();
    let cfg = conn_only(&uri);
    let inputs = ProfileInputs::from_config(&cfg);
    let worker = WorkerHandle::spawn(cfg, pw()).unwrap();
    let l = load_profiles(&worker, &inputs).unwrap();
    let mut dns = Vec::new();
    for (base, filter) in edaptor::passwd::username_searches(&l.profiles, "bbrown") {
        if let Response::Entries { entries, .. } = worker
            .request(Request::Search {
                id: 1,
                base,
                scope: SearchScope::Subtree,
                filter,
                attrs: vec!["1.1".into()],
                size_limit: None,
            })
            .unwrap()
        {
            dns.extend(entries.into_iter().map(|e| e.dn));
        }
    }
    assert_eq!(
        edaptor::passwd::resolve_outcome(dns),
        edaptor::passwd::Resolution::Unique("uid=bbrown,ou=people,dc=example,dc=org".into())
    );
}

#[test]
fn types_only_search_returns_names_without_values() {
    let uri = live!();
    let cfg = conn_only(&uri);
    let worker = WorkerHandle::spawn(cfg, pw()).unwrap();
    let resp = worker
        .request(Request::SampleSearch {
            id: 7,
            params: SampleParams {
                base: "ou=people,dc=example,dc=org".into(),
                scope: SearchScope::OneLevel,
                filter: "(objectClass=*)".into(),
                attrs: vec!["*".into()],
                size_limit: Some(5),
                types_only: true,
                time_limit: std::time::Duration::from_secs(5),
            },
        })
        .unwrap();
    let Response::Entries {
        entries, truncated, ..
    } = resp
    else {
        panic!("{resp:?}")
    };
    assert_eq!(entries.len(), 5);
    assert!(truncated, "600 users, size limit 5 -> partial");
    for e in &entries {
        assert!(e.attrs.keys().any(|k| k.eq_ignore_ascii_case("uid")));
        assert!(
            e.attrs.values().all(|v| v.is_empty()),
            "types-only must not carry values"
        );
    }
}

#[test]
fn profiles_dump_matches_the_golden_file() {
    let uri = live!();
    let r = edaptor::run_profiles(conn_only(&uri), pw(), false).unwrap();
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/golden/profiles-demo.toml"
    );
    if std::env::var_os("EDAPTOR_UPDATE_GOLDEN").is_some() {
        std::fs::write(path, &r.toml).unwrap();
    }
    let golden = std::fs::read_to_string(path).expect("regenerate with EDAPTOR_UPDATE_GOLDEN=1");
    assert_eq!(r.toml, golden, "run against a freshly started server; regenerate with EDAPTOR_UPDATE_GOLDEN=1 after reviewing");
}
