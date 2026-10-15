pub mod dns;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::{BTreeMap, BTreeSet, HashMap}, fs, net::IpAddr, path::Path};

const HEADER: &str = "# Hisoka configuration\n# Restart the service after editing: hisoka restart\n\n";

pub const DNS_PORT: u16 = 53;
pub const TTL_SECS: u32 = 30;

pub type Rules = BTreeMap<String, Rule>;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub log: LogConfig,
    pub wildcard: Rules,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LogConfig {
    pub retention_days: u32,
    pub max_mb: u32,
    pub debug: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    pub pattern: String,
    pub address: IpAddr,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self { retention_days: 7, max_mb: 10, debug: false }
    }
}

fn validate_label(label: &str) -> Result<()> {
    if label.is_empty() || label.len() > 63 {
        bail!("labels must be 1-63 characters");
    }
    if label.starts_with('-') || label.ends_with('-') {
        bail!("label '{label}' must not start or end with '-'");
    }
    if !label.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
        bail!("label '{label}' may only contain letters, digits, '-' and '_'");
    }
    Ok(())
}

pub fn normalize_pattern(raw: &str) -> Result<String> {
    let p = raw.trim().trim_end_matches('.').to_ascii_lowercase();
    if p.is_empty() {
        bail!("pattern must not be empty");
    }
    if p.len() > 253 {
        bail!("pattern is too long");
    }
    let rest = p.strip_prefix("*.").unwrap_or(&p);
    for label in rest.split('.') {
        validate_label(label)
            .with_context(|| format!("invalid pattern '{raw}' (use '*.name.tld' or 'name.tld')"))?;
    }
    Ok(p)
}

fn validate_id(id: &str) -> Result<()> {
    if id.is_empty()
        || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        bail!("rule id '{id}' may only contain letters, digits, '-' and '_'");
    }
    Ok(())
}

pub fn nrpt_namespaces(rules: &Rules) -> Vec<String> {
    let all: BTreeSet<String> = rules
        .values()
        .filter_map(|r| normalize_pattern(&r.pattern).ok())
        .map(|p| match p.strip_prefix('*') {
            Some(suffix) => suffix.to_string(),
            None => p,
        })
        .collect();
    all.iter()
        .filter(|ns| {
            !all.iter()
                .any(|s| s.starts_with('.') && s != *ns && ns.ends_with(s.as_str()))
        })
        .cloned()
        .collect()
}

// ---------- matching ----------

pub struct Matcher {
    exact: HashMap<String, (String, Rule)>,
    wild: Vec<(String, String, Rule)>,
}

impl Matcher {
    pub fn new(rules: &Rules) -> Self {
        let mut exact = HashMap::new();
        let mut wild = Vec::new();
        for (id, rule) in rules {
            let Ok(p) = normalize_pattern(&rule.pattern) else { continue };
            match p.strip_prefix('*') {
                Some(suffix) => wild.push((suffix.to_string(), id.clone(), rule.clone())),
                None => {
                    exact.insert(p, (id.clone(), rule.clone()));
                }
            }
        }
        wild.sort_by(|a, b| b.0.len().cmp(&a.0.len()));
        Self { exact, wild }
    }

    pub fn find(&self, name: &str) -> Option<(&str, &Rule)> {
        let n = name.trim_end_matches('.').to_ascii_lowercase();
        if let Some((id, rule)) = self.exact.get(&n) {
            return Some((id, rule));
        }
        self.wild
            .iter()
            .find(|(suffix, _, _)| n.len() > suffix.len() && n.ends_with(suffix.as_str()))
            .map(|(_, id, rule)| (id.as_str(), rule))
    }
}

// ---------- config ----------

impl Config {
    pub fn validate(&self) -> Result<()> {
        if self.log.retention_days == 0 {
            bail!("log.retention_days must be at least 1");
        }
        if self.log.max_mb == 0 {
            bail!("log.max_mb must be at least 1");
        }
        let mut seen: HashMap<String, &str> = HashMap::new();
        for (id, rule) in &self.wildcard {
            validate_id(id)?;
            let p = normalize_pattern(&rule.pattern)
                .with_context(|| format!("rule '{id}'"))?;
            if let Some(other) = seen.insert(p.clone(), id) {
                bail!("rules '{other}' and '{id}' have the same pattern '{p}'");
            }
        }
        Ok(())
    }

    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = fs::read_to_string(path)
            .with_context(|| format!("cannot read {}", path.display()))?;
        let cfg: Self = toml::from_str(&text)
            .with_context(|| format!("invalid config in {}", path.display()))?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Like `load`, but writes the defaults file if it is missing.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        let cfg = Self::load(path)?;
        if !path.exists() {
            cfg.save(path)?;
        }
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let body = toml::to_string_pretty(self)?;
        fs::write(path, format!("{HEADER}{body}")).with_context(|| {
            format!("cannot write {} (administrator terminal needed?)", path.display())
        })
    }

    pub fn to_toml_string(&self) -> Result<String> {
        Ok(toml::to_string_pretty(self)?)
    }

    pub fn matcher(&self) -> Matcher {
        Matcher::new(&self.wildcard)
    }

    pub fn add_rule(&mut self, id: &str, pattern: &str, address: &str) -> Result<()> {
        if self.wildcard.contains_key(id) {
            bail!("rule '{id}' already exists, use `set` to change it");
        }
        let mut next = self.clone();
        next.wildcard.insert(
            id.to_string(),
            Rule {
                pattern: normalize_pattern(pattern)?,
                address: address
                    .parse()
                    .with_context(|| format!("'{address}' is not a valid IP address"))?,
            },
        );
        next.validate()?;
        *self = next;
        Ok(())
    }

    pub fn set_rule(
        &mut self,
        id: &str,
        pattern: Option<&str>,
        address: Option<&str>,
    ) -> Result<()> {
        if pattern.is_none() && address.is_none() {
            bail!("nothing to change: pass --pattern or --address");
        }
        let mut next = self.clone();
        let rule = next
            .wildcard
            .get_mut(id)
            .with_context(|| format!("no rule named '{id}'"))?;
        if let Some(p) = pattern {
            rule.pattern = normalize_pattern(p)?;
        }
        if let Some(a) = address {
            rule.address = a
                .parse()
                .with_context(|| format!("'{a}' is not a valid IP address"))?;
        }
        next.validate()?;
        *self = next;
        Ok(())
    }

    pub fn remove_rule(&mut self, id: &str) -> Result<()> {
        self.wildcard
            .remove(id)
            .map(|_| ())
            .with_context(|| format!("no rule named '{id}'"))
    }

    pub fn set_key(&mut self, key: &str, raw: &str) -> Result<()> {
        let mut root = toml::Value::try_from(&*self)?;

        let mut parts: Vec<&str> = key.split('.').collect();
        let last = parts.pop().unwrap_or_default();
        let mut cur = &mut root;
        for p in parts {
            cur = cur.get_mut(p).with_context(|| format!("unknown key '{key}'"))?;
        }
        let slot = cur.get_mut(last).with_context(|| format!("unknown key '{key}'"))?;

        let new = match &*slot {
            toml::Value::Integer(_) => toml::Value::Integer(
                raw.parse().with_context(|| format!("'{raw}' is not a number"))?,
            ),
            toml::Value::Boolean(_) => toml::Value::Boolean(
                raw.parse().with_context(|| format!("'{raw}' is not true/false"))?,
            ),
            toml::Value::String(_) => toml::Value::String(raw.to_string()),
            _ => bail!("'{key}' is a section; use `hisoka wildcard` to edit rules"),
        };
        *slot = new;

        let updated: Config = root
            .try_into()
            .with_context(|| format!("'{raw}' is not a valid value for '{key}'"))?;
        updated.validate()?;
        *self = updated;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(rules: &[(&str, &str, &str)]) -> Config {
        let mut c = Config::default();
        for (id, p, a) in rules {
            c.add_rule(id, p, a).unwrap();
        }
        c
    }

    fn found(c: &Config, name: &str) -> Option<String> {
        c.matcher().find(name).map(|(id, _)| id.to_string())
    }

    #[test]
    fn wildcard_matches_subdomains_but_not_bare_domain() {
        let c = cfg(&[("dev", "*.test", "127.0.0.1")]);
        assert_eq!(found(&c, "a.test"), Some("dev".into()));
        assert_eq!(found(&c, "a.b.test"), Some("dev".into()));
        assert_eq!(found(&c, "test"), None);
        assert_eq!(found(&c, "nottest"), None);
        assert_eq!(found(&c, "a.test.com"), None);
    }

    #[test]
    fn most_specific_wins() {
        let c = cfg(&[
            ("dev", "*.test", "127.0.0.1"),
            ("stg", "*.staging.test", "10.0.0.5"),
            ("api", "api.staging.test", "10.0.0.9"),
        ]);
        assert_eq!(found(&c, "x.test"), Some("dev".into()));
        assert_eq!(found(&c, "x.staging.test"), Some("stg".into()));
        assert_eq!(found(&c, "api.staging.test"), Some("api".into()));
    }

    #[test]
    fn case_and_trailing_dot_are_ignored() {
        let c = cfg(&[("dev", "*.Test.", "127.0.0.1")]);
        assert_eq!(found(&c, "FOO.TEST."), Some("dev".into()));
        assert_eq!(c.wildcard["dev"].pattern, "*.test");
    }

    #[test]
    fn bad_patterns_are_rejected() {
        let mut c = Config::default();
        for bad in ["", "*", "*.", "a*.test", "*.*.test", "-a.test", "a..test", "a b.test", ".test"] {
            assert!(c.add_rule("x", bad, "127.0.0.1").is_err(), "accepted '{bad}'");
        }
    }

    #[test]
    fn bad_ids_and_addresses_are_rejected() {
        let mut c = Config::default();
        assert!(c.add_rule("bad id", "*.test", "127.0.0.1").is_err());
        assert!(c.add_rule("ok", "*.test", "not-an-ip").is_err());
        assert!(c.wildcard.is_empty());
    }

    #[test]
    fn duplicate_patterns_are_rejected_and_rolled_back() {
        let mut c = cfg(&[("a", "*.test", "127.0.0.1")]);
        assert!(c.add_rule("b", "*.TEST", "127.0.0.2").is_err());
        assert_eq!(c.wildcard.len(), 1);
    }

    #[test]
    fn set_and_remove() {
        let mut c = cfg(&[("a", "*.test", "127.0.0.1")]);
        c.set_rule("a", Some("*.dev"), Some("::1")).unwrap();
        assert_eq!(c.wildcard["a"].pattern, "*.dev");
        assert!(c.set_rule("a", None, None).is_err());
        assert!(c.set_rule("missing", Some("*.x"), None).is_err());
        c.remove_rule("a").unwrap();
        assert!(c.remove_rule("a").is_err());
    }

    #[test]
    fn toml_roundtrip() {
        let c = cfg(&[("dev", "*.test", "127.0.0.1"), ("api", "api.test", "::1")]);
        let text = toml::to_string_pretty(&c).unwrap();
        let back: Config = toml::from_str(&text).unwrap();
        back.validate().unwrap();
        assert_eq!(back.wildcard, c.wildcard);
    }

    #[test]
    fn hand_written_inline_tables_load() {
        let text = r#"
            [wildcard]
            dev = { pattern = "*.test", address = "127.0.0.1" }
        "#;
        let c: Config = toml::from_str(text).unwrap();
        c.validate().unwrap();
        assert_eq!(c.wildcard["dev"].address.to_string(), "127.0.0.1");
    }

    #[test]
    fn set_key_scalars() {
        let mut c = Config::default();
        c.set_key("log.max_mb", "20").unwrap();
        assert_eq!(c.log.max_mb, 20);
        assert!(c.set_key("log.max_mb", "0").is_err());
        assert!(c.set_key("wildcard", "x").is_err());
        assert!(c.set_key("nope.key", "1").is_err());
    }

    #[test]
    fn nrpt_namespaces_drop_redundant_entries() {
        let c = cfg(&[
            ("a", "*.test", "127.0.0.1"),
            ("b", "*.dev.test", "127.0.0.1"),
            ("c", "api.test", "127.0.0.1"),
            ("d", "solo.local", "127.0.0.1"),
        ]);
        assert_eq!(nrpt_namespaces(&c.wildcard), vec![".test", "solo.local"]);
    }
}