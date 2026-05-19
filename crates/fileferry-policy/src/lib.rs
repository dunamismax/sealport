//! Retention, forget, prune, and lifecycle policy logic.

use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    num::NonZeroU32,
    str::FromStr,
};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PolicyError {
    #[error("retention policy must include at least one keep rule")]
    EmptyPolicy,

    #[error("retention rule {name:?} is not supported")]
    UnknownRule { name: String },

    #[error("retention rule {name:?} is missing a value")]
    MissingValue { name: String },

    #[error("retention rule {name:?} is duplicated")]
    DuplicateRule { name: String },

    #[error("retention rule {name:?} has invalid count {value:?}")]
    InvalidCount { name: String, value: String },

    #[error("retention rule {name:?} has invalid tag {value:?}")]
    InvalidTag { name: String, value: String },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RetentionCount(NonZeroU32);

impl RetentionCount {
    pub fn new(value: u32) -> Result<Self, PolicyError> {
        NonZeroU32::new(value)
            .map(Self)
            .ok_or_else(|| PolicyError::InvalidCount {
                name: "count".to_owned(),
                value: value.to_string(),
            })
    }

    pub fn get(self) -> u32 {
        self.0.get()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RetentionPolicy {
    pub keep_last: Option<RetentionCount>,
    pub keep_hourly: Option<RetentionCount>,
    pub keep_daily: Option<RetentionCount>,
    pub keep_weekly: Option<RetentionCount>,
    pub keep_monthly: Option<RetentionCount>,
    pub keep_yearly: Option<RetentionCount>,
    pub keep_tags: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RetentionSnapshot {
    pub snapshot_id: String,
    pub created_at_unix_seconds: u64,
    pub tags: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RetentionAction {
    Keep,
    Forget,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RetentionDecision {
    pub snapshot_id: String,
    pub created_at_unix_seconds: u64,
    pub tags: Vec<String>,
    pub action: RetentionAction,
    pub reasons: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct RetentionPlan {
    pub decisions: Vec<RetentionDecision>,
}

impl RetentionPlan {
    pub fn candidates(&self) -> &[RetentionDecision] {
        &self.decisions
    }

    pub fn kept(&self) -> Vec<&RetentionDecision> {
        self.decisions
            .iter()
            .filter(|decision| decision.action == RetentionAction::Keep)
            .collect()
    }

    pub fn forgotten(&self) -> Vec<&RetentionDecision> {
        self.decisions
            .iter()
            .filter(|decision| decision.action == RetentionAction::Forget)
            .collect()
    }
}

impl RetentionPolicy {
    pub fn parse_spec(spec: &str) -> Result<Self, PolicyError> {
        spec.parse()
    }

    pub fn is_empty(&self) -> bool {
        self.keep_last.is_none()
            && self.keep_hourly.is_none()
            && self.keep_daily.is_none()
            && self.keep_weekly.is_none()
            && self.keep_monthly.is_none()
            && self.keep_yearly.is_none()
            && self.keep_tags.is_empty()
    }

    pub fn validate(self) -> Result<Self, PolicyError> {
        if self.is_empty() {
            Err(PolicyError::EmptyPolicy)
        } else {
            for tag in &self.keep_tags {
                parse_tag("keep-tag", tag)?;
            }
            Ok(self)
        }
    }

    pub fn plan(&self, snapshots: &[RetentionSnapshot]) -> RetentionPlan {
        let mut ordered = snapshots.to_vec();
        ordered.sort_by(|left, right| {
            right
                .created_at_unix_seconds
                .cmp(&left.created_at_unix_seconds)
                .then_with(|| right.snapshot_id.cmp(&left.snapshot_id))
        });

        let mut reasons_by_snapshot = BTreeMap::<String, BTreeSet<String>>::new();

        if let Some(count) = self.keep_last {
            for snapshot in ordered.iter().take(count.get() as usize) {
                keep_reason(&mut reasons_by_snapshot, snapshot, "keep-last");
            }
        }

        keep_periodic(
            &mut reasons_by_snapshot,
            &ordered,
            self.keep_hourly,
            "keep-hourly",
            |snapshot| (snapshot.created_at_unix_seconds / 3_600) as i64,
        );
        keep_periodic(
            &mut reasons_by_snapshot,
            &ordered,
            self.keep_daily,
            "keep-daily",
            |snapshot| (snapshot.created_at_unix_seconds / 86_400) as i64,
        );
        keep_periodic(
            &mut reasons_by_snapshot,
            &ordered,
            self.keep_weekly,
            "keep-weekly",
            |snapshot| (snapshot.created_at_unix_seconds / 604_800) as i64,
        );
        keep_periodic(
            &mut reasons_by_snapshot,
            &ordered,
            self.keep_monthly,
            "keep-monthly",
            |snapshot| {
                let (year, month, _) = civil_from_unix_seconds(snapshot.created_at_unix_seconds);
                year as i64 * 12 + i64::from(month)
            },
        );
        keep_periodic(
            &mut reasons_by_snapshot,
            &ordered,
            self.keep_yearly,
            "keep-yearly",
            |snapshot| {
                let (year, _, _) = civil_from_unix_seconds(snapshot.created_at_unix_seconds);
                year as i64
            },
        );

        for tag in &self.keep_tags {
            let reason = format!("keep-tag:{tag}");
            for snapshot in ordered
                .iter()
                .filter(|snapshot| snapshot.tags.iter().any(|candidate| candidate == tag))
            {
                keep_reason(&mut reasons_by_snapshot, snapshot, &reason);
            }
        }

        RetentionPlan {
            decisions: ordered
                .into_iter()
                .map(|snapshot| {
                    let reasons = reasons_by_snapshot
                        .remove(&snapshot.snapshot_id)
                        .map(|reasons| reasons.into_iter().collect::<Vec<_>>())
                        .unwrap_or_else(|| vec!["not-matched-by-keep-rule".to_owned()]);
                    let action = if reasons
                        .iter()
                        .any(|reason| reason == "not-matched-by-keep-rule")
                    {
                        RetentionAction::Forget
                    } else {
                        RetentionAction::Keep
                    };
                    RetentionDecision {
                        snapshot_id: snapshot.snapshot_id,
                        created_at_unix_seconds: snapshot.created_at_unix_seconds,
                        tags: snapshot.tags,
                        action,
                        reasons,
                    }
                })
                .collect(),
        }
    }
}

fn keep_reason(
    reasons_by_snapshot: &mut BTreeMap<String, BTreeSet<String>>,
    snapshot: &RetentionSnapshot,
    reason: &str,
) {
    reasons_by_snapshot
        .entry(snapshot.snapshot_id.clone())
        .or_default()
        .insert(reason.to_owned());
}

fn keep_periodic(
    reasons_by_snapshot: &mut BTreeMap<String, BTreeSet<String>>,
    ordered: &[RetentionSnapshot],
    count: Option<RetentionCount>,
    reason: &str,
    bucket: impl Fn(&RetentionSnapshot) -> i64,
) {
    let Some(count) = count else {
        return;
    };
    let mut kept_buckets = BTreeSet::new();
    for snapshot in ordered {
        let bucket = bucket(snapshot);
        if kept_buckets.insert(bucket) {
            keep_reason(reasons_by_snapshot, snapshot, reason);
            if kept_buckets.len() >= count.get() as usize {
                break;
            }
        }
    }
}

fn civil_from_unix_seconds(seconds: u64) -> (i32, u32, u32) {
    civil_from_days((seconds / 86_400) as i64)
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i32, u32, u32) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_param = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_param + 2) / 5 + 1;
    let month = month_param + if month_param < 10 { 3 } else { -9 };
    year += if month <= 2 { 1 } else { 0 };
    (year as i32, month as u32, day as u32)
}

impl FromStr for RetentionPolicy {
    type Err = PolicyError;

    fn from_str(spec: &str) -> Result<Self, Self::Err> {
        let mut policy = Self::default();
        let mut seen_scalar_rules = BTreeSet::new();

        for raw_rule in spec.split(',') {
            let raw_rule = raw_rule.trim();
            if raw_rule.is_empty() {
                continue;
            }

            let (raw_name, raw_value) =
                raw_rule
                    .split_once('=')
                    .ok_or_else(|| PolicyError::MissingValue {
                        name: raw_rule.to_owned(),
                    })?;
            let name = normalize_rule_name(raw_name);
            let value = raw_value.trim();
            if value.is_empty() {
                return Err(PolicyError::MissingValue { name });
            }

            match name.as_str() {
                "keep-last" => {
                    ensure_unique_scalar(&mut seen_scalar_rules, &name)?;
                    policy.keep_last = Some(parse_count(&name, value)?);
                }
                "keep-hourly" => {
                    ensure_unique_scalar(&mut seen_scalar_rules, &name)?;
                    policy.keep_hourly = Some(parse_count(&name, value)?);
                }
                "keep-daily" => {
                    ensure_unique_scalar(&mut seen_scalar_rules, &name)?;
                    policy.keep_daily = Some(parse_count(&name, value)?);
                }
                "keep-weekly" => {
                    ensure_unique_scalar(&mut seen_scalar_rules, &name)?;
                    policy.keep_weekly = Some(parse_count(&name, value)?);
                }
                "keep-monthly" => {
                    ensure_unique_scalar(&mut seen_scalar_rules, &name)?;
                    policy.keep_monthly = Some(parse_count(&name, value)?);
                }
                "keep-yearly" => {
                    ensure_unique_scalar(&mut seen_scalar_rules, &name)?;
                    policy.keep_yearly = Some(parse_count(&name, value)?);
                }
                "keep-tag" => policy.keep_tags.push(parse_tag(&name, value)?),
                _ => return Err(PolicyError::UnknownRule { name }),
            }
        }

        policy.validate()
    }
}

fn normalize_rule_name(name: &str) -> String {
    name.trim().replace('_', "-")
}

fn ensure_unique_scalar(seen: &mut BTreeSet<String>, name: &str) -> Result<(), PolicyError> {
    if seen.insert(name.to_owned()) {
        Ok(())
    } else {
        Err(PolicyError::DuplicateRule {
            name: name.to_owned(),
        })
    }
}

fn parse_count(name: &str, value: &str) -> Result<RetentionCount, PolicyError> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| PolicyError::InvalidCount {
            name: name.to_owned(),
            value: value.to_owned(),
        })?;
    NonZeroU32::new(parsed)
        .map(RetentionCount)
        .ok_or_else(|| PolicyError::InvalidCount {
            name: name.to_owned(),
            value: value.to_owned(),
        })
}

fn parse_tag(name: &str, value: &str) -> Result<String, PolicyError> {
    let value = value.trim();
    if value.is_empty()
        || value.contains('\0')
        || value.contains(',')
        || value.bytes().any(|byte| byte.is_ascii_control())
    {
        Err(PolicyError::InvalidTag {
            name: name.to_owned(),
            value: value.to_owned(),
        })
    } else {
        Ok(value.to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_count_based_retention_rules() {
        let policy = RetentionPolicy::parse_spec("keep-daily=14, keep-weekly=8, keep_monthly=12")
            .expect("policy");

        assert_eq!(policy.keep_daily.expect("daily").get(), 14);
        assert_eq!(policy.keep_weekly.expect("weekly").get(), 8);
        assert_eq!(policy.keep_monthly.expect("monthly").get(), 12);
        assert!(policy.keep_yearly.is_none());
    }

    #[test]
    fn parses_keep_tags_without_collapsing_duplicates() {
        let policy = RetentionPolicy::parse_spec("keep-last=5, keep-tag=laptop, keep-tag=work")
            .expect("policy");

        assert_eq!(policy.keep_last.expect("last").get(), 5);
        assert_eq!(policy.keep_tags, ["laptop", "work"]);
    }

    #[test]
    fn rejects_empty_policy() {
        let error = RetentionPolicy::parse_spec(" , ").expect_err("empty policy");

        assert_eq!(error, PolicyError::EmptyPolicy);
    }

    #[test]
    fn rejects_unknown_rules() {
        let error = RetentionPolicy::parse_spec("keep-forever=1").expect_err("unknown rule");

        assert_eq!(
            error,
            PolicyError::UnknownRule {
                name: "keep-forever".to_owned()
            }
        );
    }

    #[test]
    fn rejects_missing_values() {
        let error = RetentionPolicy::parse_spec("keep-daily=").expect_err("missing value");

        assert_eq!(
            error,
            PolicyError::MissingValue {
                name: "keep-daily".to_owned()
            }
        );
    }

    #[test]
    fn rejects_duplicate_scalar_rules() {
        let error =
            RetentionPolicy::parse_spec("keep-weekly=4, keep_weekly=5").expect_err("duplicate");

        assert_eq!(
            error,
            PolicyError::DuplicateRule {
                name: "keep-weekly".to_owned()
            }
        );
    }

    #[test]
    fn rejects_zero_and_non_numeric_counts() {
        let zero = RetentionPolicy::parse_spec("keep-daily=0").expect_err("zero");
        let text = RetentionPolicy::parse_spec("keep-daily=many").expect_err("text");

        assert_eq!(
            zero,
            PolicyError::InvalidCount {
                name: "keep-daily".to_owned(),
                value: "0".to_owned()
            }
        );
        assert_eq!(
            text,
            PolicyError::InvalidCount {
                name: "keep-daily".to_owned(),
                value: "many".to_owned()
            }
        );
    }

    #[test]
    fn rejects_malformed_tags() {
        let error = RetentionPolicy::parse_spec("keep-tag=\u{7}").expect_err("control char");

        assert_eq!(
            error,
            PolicyError::InvalidTag {
                name: "keep-tag".to_owned(),
                value: "\u{7}".to_owned()
            }
        );
    }

    #[test]
    fn validate_rejects_directly_constructed_malformed_tags() {
        let error = RetentionPolicy {
            keep_tags: vec!["bad,tag".to_owned()],
            ..RetentionPolicy::default()
        }
        .validate()
        .expect_err("invalid tag");

        assert_eq!(
            error,
            PolicyError::InvalidTag {
                name: "keep-tag".to_owned(),
                value: "bad,tag".to_owned()
            }
        );
    }

    #[test]
    fn plans_keep_last_and_forgets_older_snapshots() {
        let policy = RetentionPolicy::parse_spec("keep-last=2").expect("policy");
        let snapshots = [
            snapshot("old", 100, &[]),
            snapshot("new", 300, &[]),
            snapshot("middle", 200, &[]),
        ];

        let plan = policy.plan(&snapshots);

        let kept = plan
            .kept()
            .into_iter()
            .map(|decision| decision.snapshot_id.as_str())
            .collect::<Vec<_>>();
        let forgotten = plan
            .forgotten()
            .into_iter()
            .map(|decision| decision.snapshot_id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(kept, ["new", "middle"]);
        assert_eq!(forgotten, ["old"]);
        assert_eq!(plan.decisions[0].reasons, ["keep-last"]);
        assert_eq!(plan.decisions[2].reasons, ["not-matched-by-keep-rule"]);
    }

    #[test]
    fn plans_keep_tags_without_forgetting_matching_tags() {
        let policy = RetentionPolicy::parse_spec("keep-tag=laptop").expect("policy");
        let snapshots = [
            snapshot("untagged", 100, &[]),
            snapshot("laptop", 200, &["laptop", "work"]),
        ];

        let plan = policy.plan(&snapshots);

        assert_eq!(plan.kept()[0].snapshot_id, "laptop");
        assert_eq!(plan.kept()[0].reasons, ["keep-tag:laptop"]);
        assert_eq!(plan.forgotten()[0].snapshot_id, "untagged");
    }

    #[test]
    fn plans_daily_monthly_and_yearly_buckets_from_newest_snapshot_per_bucket() {
        let policy = RetentionPolicy::parse_spec("keep-daily=2, keep-monthly=1, keep-yearly=1")
            .expect("policy");
        let snapshots = [
            snapshot("jan-older", 1_704_067_200, &[]),
            snapshot("jan-newer", 1_704_153_600, &[]),
            snapshot("feb", 1_706_745_600, &[]),
            snapshot("mar", 1_709_251_200, &[]),
        ];

        let plan = policy.plan(&snapshots);
        let kept = plan
            .kept()
            .into_iter()
            .map(|decision| decision.snapshot_id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(kept, ["mar", "feb"]);
    }

    fn snapshot(id: &str, created_at_unix_seconds: u64, tags: &[&str]) -> RetentionSnapshot {
        RetentionSnapshot {
            snapshot_id: id.to_owned(),
            created_at_unix_seconds,
            tags: tags.iter().map(|tag| (*tag).to_owned()).collect(),
        }
    }
}
