//! Rule C (spec §2C): number ranges, computed from a full number scan at create
//! time (and eagerly by `edaptor profiles`). Pure.

use crate::detect::model::{Evidence, SampleEntry};
use crate::detect::private::PrivateIndex;
use crate::detect::{dn_eq, MIN_SAMPLE};

/// Neighbouring values more than this apart start a new block.
pub const BLOCK_GAP: u64 = 1000;
/// Rule D (§2D): first number when the space is empty. Client machines hand out
/// 1000 and up to local users, so LDAP numbers start higher.
pub const ASSUMED_MIN: u64 = 10000;
/// Upper end of the highest block: `max(OPEN_END, MIN + 9999)`.
pub const OPEN_END: u64 = 60000;
/// The allocation scan (subtree under `base_dn`, no size limit).
pub const SCAN_FILTER: &str = "(|(uidNumber=*)(gidNumber=*))";
pub const SCAN_ATTRS: &[&str] = &["objectClass", "cn", "uid", "uidNumber", "gidNumber"];

/// What a detected `{next:…}` allocates and which entries are "its" values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeSpec {
    /// `uidNumber` or `gidNumber`.
    pub attr: String,
    /// The profile's `search_base`.
    pub container: String,
    /// The profile's structural class.
    pub structural: String,
    /// B2 applied: uidNumber and gidNumber form one number space.
    pub unified: bool,
    /// posixGroup profile: private groups are not its values.
    pub exclude_private: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RangeReport {
    pub min: u64,
    pub max: u64,
    pub next: u64,
    /// Lowest and highest number in use in the chosen block; `None` when the
    /// space is empty (rule D start).
    pub in_use: Option<(u64, u64)>,
    pub next_block: Option<u64>,
    pub exhausted: bool,
    pub evidence: Evidence,
}

impl RangeReport {
    /// `{next:MIN-MAX}`.
    pub fn template(&self) -> String {
        format!("{{next:{}-{}}}", self.min, self.max)
    }

    /// `in use 5000-5016; next block at 8000[; pool exhausted]`.
    pub fn describe(&self) -> String {
        let Some((lo, hi)) = self.in_use else {
            return format!("no numbers in use; useradd-style start at {ASSUMED_MIN}");
        };
        let mut s = format!("in use {lo}-{hi}");
        match self.next_block {
            Some(b) => s.push_str(&format!("; next block at {b}")),
            None => s.push_str("; no higher block"),
        }
        if self.exhausted {
            s.push_str("; pool exhausted");
        }
        s
    }
}

/// Numbers of `attr`; anything that is not a valid 32-bit POSIX id (text, empty,
/// negative, too large) is ignored.
fn nums(e: &SampleEntry, attr: &str) -> Vec<u64> {
    e.values(attr)
        .iter()
        .filter_map(|v| v.trim().parse::<u32>().ok())
        .map(u64::from)
        .collect()
}

fn split_blocks(sorted: &[u64]) -> Vec<(u64, u64)> {
    let mut out: Vec<(u64, u64)> = Vec::new();
    for &v in sorted {
        match out.last_mut() {
            Some(b) if v - b.1 <= BLOCK_GAP => b.1 = v,
            _ => out.push((v, v)),
        }
    }
    out
}

/// Apply rule C to a full scan.
pub fn detect_range(spec: &RangeSpec, entries: &[SampleEntry]) -> Result<RangeReport, String> {
    let index = PrivateIndex::new(entries.iter());
    let mut space: Vec<u64> = Vec::new();
    for e in entries {
        let is_account = e.has_class("posixAccount");
        if spec.unified {
            if is_account {
                space.extend(nums(e, "uidNumber"));
            }
            space.extend(nums(e, "gidNumber"));
        } else if spec.attr.eq_ignore_ascii_case("uidNumber") {
            space.extend(nums(e, "uidNumber"));
        } else if !is_account {
            space.extend(nums(e, &spec.attr));
        }
    }
    space.sort_unstable();
    space.dedup();
    let mine: Vec<(u64, &str)> = entries
        .iter()
        .filter(|e| e.parent().is_some_and(|p| dn_eq(p, &spec.container)))
        .filter(|e| e.has_class(&spec.structural))
        .filter(|e| !(spec.exclude_private && index.is_private(e)))
        .flat_map(|e| {
            nums(e, &spec.attr)
                .into_iter()
                .map(move |n| (n, e.dn.as_str()))
        })
        .collect();
    if space.is_empty() {
        // Rule D: nothing in use yet.
        return Ok(RangeReport {
            min: ASSUMED_MIN,
            max: OPEN_END,
            next: ASSUMED_MIN,
            in_use: None,
            next_block: None,
            exhausted: false,
            evidence: Evidence::new(0, 0).with_note("no numbers in use; useradd-style start"),
        });
    }
    let blocks = split_blocks(&space);
    // The profile's block holds most of its own values; a profile without values
    // yet takes the block holding most numbers of the space. Ties: the lower block.
    let in_block = |vals: &mut dyn Iterator<Item = u64>, b: &(u64, u64)| {
        vals.filter(|n| *n >= b.0 && *n <= b.1).count()
    };
    let count = |b: &(u64, u64)| {
        if mine.is_empty() {
            in_block(&mut space.iter().copied(), b)
        } else {
            in_block(&mut mine.iter().map(|(n, _)| *n), b)
        }
    };
    let (bi, block) = blocks
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| count(a).cmp(&count(b)).then_with(|| b.0.cmp(&a.0)))
        .map(|(i, b)| (i, *b))
        .expect("the space is not empty, so there is a block");
    // The 3-entry threshold counts only for exceptions (§2D).
    let exceptions: Vec<String> = if mine.len() >= MIN_SAMPLE {
        mine.iter()
            .filter(|(n, _)| *n < block.0 || *n > block.1)
            .map(|(_, dn)| dn.to_string())
            .collect()
    } else {
        Vec::new()
    };
    let matched = mine
        .iter()
        .filter(|(n, _)| *n >= block.0 && *n <= block.1)
        .count();
    let min = block.0 / 1000 * 1000;
    let next_block = blocks.get(bi + 1).map(|b| b.0);
    let max = match next_block {
        Some(lo) => lo / 1000 * 1000 - 1,
        None => OPEN_END.max(min + 9999),
    };
    let next = block.1 + 1;
    Ok(RangeReport {
        min,
        max,
        next,
        in_use: Some(block),
        next_block,
        exhausted: next > max,
        evidence: Evidence::new(matched, mine.len()).with_exceptions(exceptions),
    })
}

/// The number to allocate: `max(in use in block) + 1`; refuses on an exhausted pool
/// with the same message `{next:MIN-MAX}` uses.
pub fn allocate(spec: &RangeSpec, entries: &[SampleEntry]) -> Result<(u64, Evidence), String> {
    let r = detect_range(spec, entries)?;
    if r.exhausted {
        return Err(format!("number pool {}-{} is exhausted", r.min, r.max));
    }
    Ok((r.next, r.evidence))
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::fixtures::{argus_sample, e};

    fn all_entries(s: &crate::detect::model::Sample) -> Vec<SampleEntry> {
        s.containers
            .iter()
            .flat_map(|c| c.entries.clone())
            .collect()
    }
    fn spec(attr: &str, container: &str, structural: &str, unified: bool, excl: bool) -> RangeSpec {
        RangeSpec {
            attr: attr.into(),
            container: container.into(),
            structural: structural.into(),
            unified,
            exclude_private: excl,
        }
    }
    fn acct(i: u64, num: &str) -> SampleEntry {
        e(
            &format!("uid=a{i},ou=p,dc=x"),
            &[
                ("objectClass", &["inetOrgPerson", "posixAccount"]),
                ("uid", &[&format!("a{i}")]),
                ("uidNumber", &[num]),
                ("gidNumber", &["100"]),
            ],
        )
    }

    #[test]
    fn argus_users_and_shared_groups() {
        let all = all_entries(&argus_sample());
        let users = detect_range(
            &spec(
                "uidNumber",
                "ou=people,dc=argus,dc=ch",
                "inetOrgPerson",
                true,
                false,
            ),
            &all,
        )
        .unwrap();
        assert_eq!((users.min, users.max), (5000, 7999));
        assert_eq!(users.next, 5021, "staff (5020) is in the user block");
        assert_eq!(users.next_block, Some(8000));
        assert_eq!(users.template(), "{next:5000-7999}");
        let groups = detect_range(
            &spec(
                "gidNumber",
                "ou=groups,dc=argus,dc=ch",
                "posixGroup",
                true,
                true,
            ),
            &all,
        )
        .unwrap();
        assert_eq!((groups.min, groups.max, groups.next), (8000, 60000, 8003));
        assert_eq!(groups.evidence.ratio(), "3/4");
        assert_eq!(
            groups.evidence.exceptions,
            vec!["cn=staff,ou=groups,dc=argus,dc=ch"]
        );
    }

    #[test]
    fn neighbouring_block_caps_max() {
        let v = vec![
            acct(1, "5000"),
            acct(2, "5001"),
            acct(3, "5003"),
            acct(4, "6200"),
        ];
        let r = detect_range(
            &spec("uidNumber", "ou=p,dc=x", "inetOrgPerson", false, false),
            &v,
        )
        .unwrap();
        assert_eq!((r.min, r.max, r.next), (5000, 5999, 5004));
    }

    #[test]
    fn a_block_at_65534_yields_a_valid_range() {
        let v = vec![acct(1, "65532"), acct(2, "65533"), acct(3, "65534")];
        let r = detect_range(
            &spec("uidNumber", "ou=p,dc=x", "inetOrgPerson", false, false),
            &v,
        )
        .unwrap();
        assert!(r.min <= r.max);
        assert_eq!((r.min, r.max, r.next), (65000, 74999, 65535));
    }

    #[test]
    fn exhausted_pool_is_reported_and_allocation_refuses() {
        let v: Vec<SampleEntry> = (0..=20)
            .map(|i| acct(i, &(50000 + i * 500).to_string()))
            .collect();
        let s = spec("uidNumber", "ou=p,dc=x", "inetOrgPerson", false, false);
        let r = detect_range(&s, &v).unwrap();
        assert_eq!((r.min, r.max, r.next), (50000, 60000, 60001));
        assert!(r.exhausted);
        assert!(r.describe().contains("pool exhausted"));
        assert_eq!(
            allocate(&s, &v).unwrap_err(),
            "number pool 50000-60000 is exhausted"
        );
    }

    #[test]
    fn non_unified_uid_space_ignores_account_gids() {
        let v = vec![acct(1, "10000"), acct(2, "10001"), acct(3, "10002")];
        let (n, ev) = allocate(
            &spec("uidNumber", "ou=p,dc=x", "inetOrgPerson", false, false),
            &v,
        )
        .unwrap();
        assert_eq!(n, 10003);
        assert_eq!(ev.ratio(), "3/3");
    }

    #[test]
    fn private_group_found_when_uid_differs_from_cn() {
        let v = vec![
            e(
                "cn=Alice Smith,ou=p,dc=x",
                &[
                    ("objectClass", &["posixAccount"]),
                    ("uid", &["alice"]),
                    ("cn", &["Alice Smith"]),
                    ("uidNumber", &["7000"]),
                    ("gidNumber", &["7000"]),
                ],
            ),
            e(
                "cn=alice,ou=g,dc=x",
                &[
                    ("objectClass", &["posixGroup"]),
                    ("cn", &["alice"]),
                    ("gidNumber", &["7000"]),
                ],
            ),
            e(
                "cn=a,ou=g,dc=x",
                &[
                    ("objectClass", &["posixGroup"]),
                    ("cn", &["a"]),
                    ("gidNumber", &["9000"]),
                ],
            ),
            e(
                "cn=b,ou=g,dc=x",
                &[
                    ("objectClass", &["posixGroup"]),
                    ("cn", &["b"]),
                    ("gidNumber", &["9001"]),
                ],
            ),
            e(
                "cn=c,ou=g,dc=x",
                &[
                    ("objectClass", &["posixGroup"]),
                    ("cn", &["c"]),
                    ("gidNumber", &["9002"]),
                ],
            ),
        ];
        let r = detect_range(
            &spec("gidNumber", "ou=g,dc=x", "posixGroup", true, true),
            &v,
        )
        .unwrap();
        assert_eq!(
            r.evidence.ratio(),
            "3/3",
            "the private group is not a value of the group profile"
        );
        assert_eq!((r.min, r.next), (9000, 9003));
    }

    #[test]
    fn an_empty_space_starts_useradd_style_at_10000() {
        let r = detect_range(
            &spec("uidNumber", "ou=p,dc=x", "inetOrgPerson", true, false),
            &[],
        )
        .unwrap();
        assert_eq!((r.min, r.max, r.next), (10000, 60000, 10000));
        assert_eq!(r.in_use, None);
        assert_eq!(r.template(), "{next:10000-60000}");
        assert!(
            r.describe().contains("no numbers in use"),
            "{}",
            r.describe()
        );
        assert_eq!(
            allocate(
                &spec("gidNumber", "ou=g,dc=x", "posixGroup", true, true),
                &[]
            )
            .unwrap()
            .0,
            10000
        );
    }

    #[test]
    fn one_or_two_values_continue_their_block() {
        // argus-like start: one user at 5000 → 5001, not 10000.
        let r = detect_range(
            &spec("uidNumber", "ou=p,dc=x", "inetOrgPerson", false, false),
            &[acct(1, "5000")],
        )
        .unwrap();
        assert_eq!((r.min, r.max, r.next), (5000, 60000, 5001));
        // Two values in different blocks: no exceptions below the 3-entry threshold.
        let v = vec![acct(1, "5000"), acct(2, "9000")];
        let r = detect_range(
            &spec("uidNumber", "ou=p,dc=x", "inetOrgPerson", false, false),
            &v,
        )
        .unwrap();
        assert_eq!((r.min, r.next), (5000, 5001));
        assert!(r.evidence.exceptions.is_empty());
    }

    #[test]
    fn a_profile_without_own_values_uses_the_fullest_block() {
        // New group profile; the unified space already holds user numbers 5000, 5001.
        let v = vec![acct(1, "5000"), acct(2, "5001")];
        let r = detect_range(
            &spec("gidNumber", "ou=g,dc=x", "posixGroup", true, true),
            &v,
        )
        .unwrap();
        assert_eq!(
            (r.min, r.next),
            (5000, 5002),
            "100 is the accounts' gid, counted in a unified space"
        );
    }

    #[test]
    fn garbage_numbers_are_ignored() {
        let mut v = vec![acct(1, "10000"), acct(2, "10001"), acct(3, "10002")];
        v.push(acct(4, "abc"));
        v.push(acct(5, "-7"));
        v.push(acct(6, ""));
        v.push(e(
            "uid=a7,ou=p,dc=x",
            &[
                ("objectClass", &["inetOrgPerson", "posixAccount"]),
                ("uidNumber", &["10003", "99999"]),
            ],
        ));
        let r = detect_range(
            &spec("uidNumber", "ou=p,dc=x", "inetOrgPerson", false, false),
            &v,
        )
        .unwrap();
        assert_eq!(r.min, 10000);
        assert_eq!(r.next, 10004);
    }

    #[test]
    fn out_of_range_numbers_are_ignored_without_overflow() {
        let v = vec![
            acct(1, "10000"),
            acct(2, "10001"),
            acct(3, "18446744073709551615"),
            acct(4, "4294967296"),
        ];
        let r = detect_range(
            &spec("uidNumber", "ou=p,dc=x", "inetOrgPerson", false, false),
            &v,
        )
        .unwrap();
        assert_eq!((r.min, r.next), (10000, 10002));
        // A lone absurd value must not overflow `next` or `max`.
        let lone = vec![acct(1, "18446744073709551615")];
        let r = detect_range(
            &spec("uidNumber", "ou=p,dc=x", "inetOrgPerson", false, false),
            &lone,
        )
        .unwrap();
        assert_eq!((r.min, r.next), (ASSUMED_MIN, ASSUMED_MIN));
    }
}
