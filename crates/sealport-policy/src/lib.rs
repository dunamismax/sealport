//! Retention, forget, prune, and lifecycle policy logic.

use std::{collections::BTreeSet, num::NonZeroU32, str::FromStr};

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
            Ok(self)
        }
    }
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
}
