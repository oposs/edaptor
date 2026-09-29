//! Sampling (spec §1.1): containers, one-level samples with values and
//! types-only presence, cross-container private-group lookups. Talks to LDAP
//! through `Searcher`, so the logic is unit-tested with a fake.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use crate::detect::model::{ContainerSample, Sample, SampleEntry};
use crate::detect::{LOOKUP_BATCH, MAX_CONTAINERS, SAMPLE_ATTRS, SAMPLE_SIZE};
use crate::ldap::worker::{Request, Response, SampleParams, SearchScope, WorkerHandle};
use crate::workflows::pick_state::escape_filter;

pub const HAS_SUBORDINATES_FILTER: &str = "(hasSubordinates=TRUE)";
pub const FALLBACK_FILTER: &str = "(|(objectClass=organizationalUnit)(objectClass=organization)(objectClass=domain)(objectClass=container))";

pub trait Searcher {
    /// Entries plus `partial` (a limit or timeout cut the result short).
    fn search(&mut self, q: &SampleParams) -> Result<(Vec<SampleEntry>, bool), String>;
}

pub struct WorkerSearcher<'a>(pub &'a WorkerHandle);

impl Searcher for WorkerSearcher<'_> {
    fn search(&mut self, q: &SampleParams) -> Result<(Vec<SampleEntry>, bool), String> {
        match self.0.request(Request::SampleSearch {
            id: 0,
            params: q.clone(),
        }) {
            Ok(Response::Entries {
                entries, truncated, ..
            }) => Ok((entries.iter().map(SampleEntry::from).collect(), truncated)),
            Ok(Response::SearchError { msg, .. }) => Err(msg),
            Ok(other) => Err(format!("unexpected worker response {other:?}")),
            Err(e) => Err(e.to_string()),
        }
    }
}

/// The time left of the detection budget.
pub struct Budget {
    deadline: Instant,
}

impl Budget {
    pub fn new(total: Duration) -> Self {
        Budget {
            deadline: Instant::now() + total,
        }
    }

    /// `None` once the budget is used up.
    pub fn remaining(&self) -> Option<Duration> {
        let r = self.deadline.saturating_duration_since(Instant::now());
        (!r.is_zero()).then_some(r)
    }
}

fn params(
    base: &str,
    scope: SearchScope,
    filter: &str,
    attrs: Vec<String>,
    size: Option<i32>,
    types_only: bool,
    time_limit: Duration,
) -> SampleParams {
    SampleParams {
        base: base.to_string(),
        scope,
        filter: filter.to_string(),
        attrs,
        size_limit: size,
        types_only,
        time_limit,
    }
}

const NO_BUDGET: &str = "the detection budget was used up before the container search";

/// The base is absent or unreadable for this bind (anonymous or ACL-restricted
/// view, spec §4): LDAP result 32 or 50. The worker's messages always carry
/// `(LDAP <rc>)` (see `result_code_message`), which is what this matches.
fn nothing_visible(msg: &str) -> bool {
    msg.contains("(LDAP 32)") || msg.contains("(LDAP 50)")
}

fn invisible(mut out: Sample, msg: &str) -> Sample {
    out.notes
        .push(format!("nothing visible under the base ({msg})"));
    out
}

pub fn sample(s: &mut dyn Searcher, base_dn: &str, budget: &Budget) -> Result<Sample, String> {
    let mut out = Sample {
        base_dn: base_dn.to_string(),
        ..Default::default()
    };
    let t = budget.remaining().ok_or(NO_BUDGET)?;
    let one = vec!["1.1".to_string()];
    let first = s.search(&params(
        base_dn,
        SearchScope::Subtree,
        HAS_SUBORDINATES_FILTER,
        one.clone(),
        None,
        false,
        t,
    ));
    let (found, partial) = match first {
        Ok(r) => r,
        Err(e) if nothing_visible(&e) => return Ok(invisible(out, &e)),
        Err(e) => {
            out.notes.push(format!(
                "container search {HAS_SUBORDINATES_FILTER} failed ({e}); used the objectClass fallback"
            ));
            let t = budget.remaining().ok_or(NO_BUDGET)?;
            match s.search(&params(
                base_dn,
                SearchScope::Subtree,
                FALLBACK_FILTER,
                one,
                None,
                false,
                t,
            )) {
                Ok(r) => r,
                Err(e) if nothing_visible(&e) => return Ok(invisible(out, &e)),
                Err(e) => return Err(format!("container search failed: {e}")),
            }
        }
    };
    if partial {
        out.incomplete = true;
        out.notes
            .push("the container search hit a limit; some containers may be missing".to_string());
    }
    let mut dns: Vec<String> = Vec::new();
    for c in found {
        if !dns.iter().any(|d| crate::detect::dn_eq(d, &c.dn)) {
            dns.push(c.dn);
        }
    }
    if dns.len() > MAX_CONTAINERS {
        out.incomplete = true;
        out.notes.push(format!(
            "sampled the first {MAX_CONTAINERS} containers; {} more containers skipped",
            dns.len() - MAX_CONTAINERS
        ));
        dns.truncate(MAX_CONTAINERS);
    }
    let attrs: Vec<String> = SAMPLE_ATTRS.iter().map(|a| a.to_string()).collect();
    for (i, dn) in dns.iter().enumerate() {
        let Some(t) = budget.remaining() else {
            out.incomplete = true;
            out.notes.push(format!(
                "detection budget used up; skipped {} containers (partial)",
                dns.len() - i
            ));
            break;
        };
        let (entries, mut partial) = match s.search(&params(
            dn,
            SearchScope::OneLevel,
            "(objectClass=*)",
            attrs.clone(),
            Some(SAMPLE_SIZE),
            false,
            t,
        )) {
            Ok(r) => r,
            Err(e) => {
                out.incomplete = true;
                out.notes.push(format!("sampling {dn} failed: {e}"));
                continue;
            }
        };
        let mut present: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        match budget.remaining() {
            Some(t) => match s.search(&params(
                dn,
                SearchScope::OneLevel,
                "(objectClass=*)",
                vec!["*".to_string()],
                Some(SAMPLE_SIZE),
                true,
                t,
            )) {
                Ok((typed, p)) => {
                    partial |= p;
                    for te in typed {
                        // Same key form as `ContainerSample::present_attrs`.
                        present.insert(
                            te.dn.to_lowercase(),
                            te.attrs.keys().map(|k| k.to_lowercase()).collect(),
                        );
                    }
                }
                Err(e) => out
                    .notes
                    .push(format!("attribute presence for {dn} failed: {e}")),
            },
            None => partial = true,
        }
        out.containers.push(ContainerSample {
            dn: dn.clone(),
            entries,
            present,
            partial,
        });
    }
    out.group_ou = find_group_ou(s, &out, base_dn, budget);
    lookups(s, &mut out, budget);
    Ok(out)
}

/// `ou=groups` directly under the base, if it exists. An empty OU has no
/// subordinates, so it is not a sampled container; look for it among the
/// sampled entries first, then with one base-scope read.
fn find_group_ou(
    s: &mut dyn Searcher,
    out: &Sample,
    base_dn: &str,
    budget: &Budget,
) -> Option<String> {
    let want = format!("ou=groups,{base_dn}");
    let seen = out
        .containers
        .iter()
        .any(|c| crate::detect::dn_eq(&c.dn, &want))
        || out
            .containers
            .iter()
            .flat_map(|c| c.entries.iter())
            .any(|e| crate::detect::dn_eq(&e.dn, &want));
    if seen {
        return Some(want);
    }
    let t = budget.remaining()?;
    match s.search(&params(
        &want,
        SearchScope::Base,
        "(objectClass=*)",
        vec!["1.1".to_string()],
        None,
        false,
        t,
    )) {
        Ok((found, _)) if !found.is_empty() => Some(want),
        _ => None,
    }
}

/// Forward (`posixGroup` by sampled `uid`) and reverse (`posixAccount` by
/// sampled group `cn`) lookups, batched at `LOOKUP_BATCH`.
fn lookups(s: &mut dyn Searcher, out: &mut Sample, budget: &Budget) {
    let mut uids: Vec<String> = Vec::new();
    let mut cns: Vec<String> = Vec::new();
    for e in out.containers.iter().flat_map(|c| c.entries.iter()) {
        if e.has_class("posixAccount") {
            if let Some(u) = e.first("uid") {
                uids.push(u.to_string());
            }
        }
        if e.has_class("posixGroup") {
            if let Some(c) = e.first("cn") {
                cns.push(c.to_string());
            }
        }
    }
    let jobs = [
        (
            "posixGroup",
            "cn",
            uids,
            vec!["objectClass", "cn", "gidNumber", "memberUid"],
        ),
        (
            "posixAccount",
            "uid",
            cns,
            vec!["objectClass", "uid", "uidNumber", "gidNumber"],
        ),
    ];
    for (class, key, values, attrs) in jobs {
        for chunk in values.chunks(LOOKUP_BATCH) {
            let Some(t) = budget.remaining() else {
                out.lookup_error = Some("the detection budget was used up".to_string());
                return;
            };
            let ors: String = chunk
                .iter()
                .map(|v| format!("({key}={})", escape_filter(v)))
                .collect();
            let filter = format!("(&(objectClass={class})(|{ors}))");
            let attrs = attrs.iter().map(|a| a.to_string()).collect();
            let base = out.base_dn.clone();
            match s.search(&params(
                &base,
                SearchScope::Subtree,
                &filter,
                attrs,
                None,
                false,
                t,
            )) {
                Ok((found, false)) => {
                    if class == "posixGroup" {
                        out.groups.extend(found);
                    } else {
                        out.accounts.extend(found);
                    }
                }
                Ok((_, true)) => {
                    out.lookup_error = Some(format!("the {class} lookup hit a server limit"));
                    return;
                }
                Err(e) => {
                    out.lookup_error = Some(format!("the {class} lookup failed: {e}"));
                    return;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::fixtures::e;

    type Reply = Result<(Vec<SampleEntry>, bool), String>;
    struct Fake {
        calls: Vec<SampleParams>,
        reply: Box<dyn FnMut(&SampleParams) -> Reply>,
    }
    impl Searcher for Fake {
        fn search(&mut self, q: &SampleParams) -> Reply {
            self.calls.push(q.clone());
            (self.reply)(q)
        }
    }
    fn fake(reply: impl FnMut(&SampleParams) -> Reply + 'static) -> Fake {
        Fake {
            calls: Vec::new(),
            reply: Box::new(reply),
        }
    }
    fn budget() -> Budget {
        Budget::new(std::time::Duration::from_secs(30))
    }
    fn user(i: usize) -> SampleEntry {
        e(
            &format!("uid=u{i},ou=p,dc=x"),
            &[
                ("objectClass", &["posixAccount"]),
                ("uid", &[&format!("u{i}")]),
            ],
        )
    }

    #[test]
    fn samples_each_container_with_values_and_types_only() {
        let mut f = fake(|q| match q.filter.as_str() {
            HAS_SUBORDINATES_FILTER => Ok((vec![e("ou=p,dc=x", &[])], false)),
            "(objectClass=*)" if q.types_only => Ok((
                vec![e("uid=u1,ou=p,dc=x", &[("jpegPhoto", &[]), ("uid", &[])])],
                false,
            )),
            "(objectClass=*)" => Ok((vec![user(1)], true)),
            _ => Ok((vec![], false)),
        });
        let s = sample(&mut f, "dc=x", &budget()).unwrap();
        assert_eq!(s.containers.len(), 1);
        let c = &s.containers[0];
        assert!(c.partial, "size-limited sample is partial");
        assert!(
            !s.incomplete,
            "a truncated container still yields its profile: not incomplete"
        );
        assert!(c.present["uid=u1,ou=p,dc=x"].contains("jpegphoto"));
        let values = f
            .calls
            .iter()
            .find(|q| q.base == "ou=p,dc=x" && !q.types_only)
            .unwrap();
        assert_eq!(values.scope, SearchScope::OneLevel);
        assert_eq!(values.size_limit, Some(SAMPLE_SIZE));
        assert!(!values
            .attrs
            .iter()
            .any(|a| a.eq_ignore_ascii_case("userPassword")));
        let types = f.calls.iter().find(|q| q.types_only).unwrap();
        assert_eq!(types.attrs, vec!["*"]);
    }

    #[test]
    fn presence_keys_are_lowercased_dns_as_present_attrs_expects() {
        let mut f = fake(|q| match q.filter.as_str() {
            HAS_SUBORDINATES_FILTER => Ok((vec![e("ou=p,dc=x", &[])], false)),
            "(objectClass=*)" if q.types_only => {
                Ok((vec![e("uid=U1,ou=P,dc=x", &[("jpegPhoto", &[])])], false))
            }
            "(objectClass=*)" => Ok((vec![e("uid=U1,ou=P,dc=x", &[])], false)),
            _ => Ok((vec![], false)),
        });
        let s = sample(&mut f, "dc=x", &budget()).unwrap();
        let c = &s.containers[0];
        let attrs = c.present_attrs(&c.entries[0]);
        assert!(
            attrs.contains("jpegphoto"),
            "presence map must be hit, not the fallback"
        );
    }

    #[test]
    fn a_search_hitting_its_limit_yields_a_partial_sample_with_a_note() {
        // Container search cut short (time limit), then one container whose
        // value search times out with partial data: no error, partial flag, note.
        let mut f = fake(|q| match q.filter.as_str() {
            HAS_SUBORDINATES_FILTER => Ok((vec![e("ou=p,dc=x", &[])], true)),
            "(objectClass=*)" if !q.types_only => Ok((vec![user(1)], true)),
            _ => Ok((vec![], false)),
        });
        let s = sample(&mut f, "dc=x", &budget()).unwrap();
        assert_eq!(s.containers.len(), 1);
        assert!(s.containers[0].partial);
        assert!(s.notes.iter().any(|n| n.contains("limit")));
    }

    #[test]
    fn a_budget_that_runs_out_mid_way_keeps_what_was_sampled() {
        let mut f = fake(|q| match q.filter.as_str() {
            HAS_SUBORDINATES_FILTER => Ok((vec![e("ou=a,dc=x", &[]), e("ou=b,dc=x", &[])], false)),
            _ => Ok((vec![], false)),
        });
        // A budget that is gone right after the container search.
        let b = Budget::new(std::time::Duration::from_millis(60));
        let mut first = true;
        let mut g = fake(move |q| {
            if first {
                first = false;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            (f.reply)(q)
        });
        let s = sample(&mut g, "dc=x", &b).unwrap();
        assert!(s.containers.is_empty());
        assert!(s.notes.iter().any(|n| n.contains("budget")));
        assert!(
            s.incomplete,
            "skipped containers make the sample incomplete"
        );
    }

    #[test]
    fn rejected_container_filter_falls_back_with_a_note() {
        let mut f = fake(|q| match q.filter.as_str() {
            HAS_SUBORDINATES_FILTER => Err("unwilling to perform".into()),
            FALLBACK_FILTER => Ok((vec![e("ou=p,dc=x", &[])], false)),
            _ => Ok((vec![], false)),
        });
        let s = sample(&mut f, "dc=x", &budget()).unwrap();
        assert_eq!(s.containers.len(), 1);
        assert!(s.notes.iter().any(|n| n.contains("fallback")));
    }

    #[test]
    fn no_such_object_or_no_access_on_the_base_is_an_empty_sample() {
        for msg in [
            "searching dc=x: No such object (LDAP 32)",
            "searching dc=x: Insufficient access rights (LDAP 50)",
        ] {
            let mut f = fake(move |_| Err(msg.to_string()));
            let s = sample(&mut f, "dc=x", &budget()).expect("nothing visible is not an error");
            assert!(s.containers.is_empty());
            assert!(
                s.notes.iter().any(|n| n.contains("nothing visible")),
                "{msg}"
            );
        }
        // The fallback filter hitting the same wall is the same outcome.
        let mut f = fake(|q| match q.filter.as_str() {
            HAS_SUBORDINATES_FILTER => Err("unwilling to perform".into()),
            _ => Err("No such object (LDAP 32)".into()),
        });
        assert!(sample(&mut f, "dc=x", &budget())
            .unwrap()
            .containers
            .is_empty());
        // Anything else stays an error.
        let mut f = fake(|_| Err("Can't contact LDAP server".into()));
        assert!(sample(&mut f, "dc=x", &budget()).is_err());
    }

    #[test]
    fn nothing_visible_is_an_empty_sample_not_an_error() {
        let mut f = fake(|_| Ok((vec![], false)));
        let s = sample(&mut f, "dc=x", &budget()).unwrap();
        assert!(s.containers.is_empty());
        assert!(s.lookup_error.is_none());
    }

    #[test]
    fn container_cap_is_noted() {
        let mut f = fake(|q| match q.filter.as_str() {
            HAS_SUBORDINATES_FILTER => Ok((
                (0..105).map(|i| e(&format!("ou=c{i},dc=x"), &[])).collect(),
                false,
            )),
            _ => Ok((vec![], false)),
        });
        let s = sample(&mut f, "dc=x", &budget()).unwrap();
        assert_eq!(s.containers.len(), MAX_CONTAINERS);
        assert!(s.notes.iter().any(|n| n.contains("5 more containers")));
    }

    #[test]
    fn exhausted_budget_fails_before_the_first_search() {
        let mut f = fake(|_| Ok((vec![], false)));
        assert!(sample(&mut f, "dc=x", &Budget::new(std::time::Duration::ZERO)).is_err());
        assert!(f.calls.is_empty());
    }

    #[test]
    fn an_empty_groups_ou_is_found_by_a_base_read() {
        let mut f = fake(|q| match (q.filter.as_str(), q.scope) {
            (HAS_SUBORDINATES_FILTER, _) => Ok((vec![], false)),
            (_, SearchScope::Base) if q.base == "ou=groups,dc=x" => {
                Ok((vec![e("ou=groups,dc=x", &[])], false))
            }
            _ => Ok((vec![], false)),
        });
        assert_eq!(
            sample(&mut f, "dc=x", &budget())
                .unwrap()
                .group_ou
                .as_deref(),
            Some("ou=groups,dc=x")
        );
        let mut none = fake(|_| Ok((vec![], false)));
        assert_eq!(sample(&mut none, "dc=x", &budget()).unwrap().group_ou, None);
    }

    #[test]
    fn lookups_are_batched_and_escaped() {
        let mut f = fake(|q| match q.filter.as_str() {
            HAS_SUBORDINATES_FILTER => Ok((vec![e("ou=p,dc=x", &[])], false)),
            "(objectClass=*)" if !q.types_only => Ok((
                (0..120)
                    .map(user)
                    .chain([e(
                        "uid=x,ou=p,dc=x",
                        &[("objectClass", &["posixAccount"]), ("uid", &["a*b"])],
                    )])
                    .collect(),
                false,
            )),
            _ => Ok((vec![], false)),
        });
        sample(&mut f, "dc=x", &budget()).unwrap();
        let lookups: Vec<&SampleParams> = f
            .calls
            .iter()
            .filter(|q| q.filter.starts_with("(&(objectClass=posixGroup)"))
            .collect();
        assert_eq!(lookups.len(), 3, "121 uids → 3 batches of ≤ 50");
        assert!(lookups.iter().any(|q| q.filter.contains(r"(cn=a\2ab)")));
        assert!(lookups
            .iter()
            .all(|q| q.base == "dc=x" && q.scope == SearchScope::Subtree));
    }

    #[test]
    fn a_failed_lookup_is_recorded() {
        let mut f = fake(|q| match q.filter.as_str() {
            HAS_SUBORDINATES_FILTER => Ok((vec![e("ou=p,dc=x", &[])], false)),
            "(objectClass=*)" if !q.types_only => Ok(((0..3).map(user).collect(), false)),
            f if f.starts_with("(&") => Err("insufficient access".into()),
            _ => Ok((vec![], false)),
        });
        let s = sample(&mut f, "dc=x", &budget()).unwrap();
        assert!(s
            .lookup_error
            .as_deref()
            .unwrap()
            .contains("insufficient access"));
    }
}
