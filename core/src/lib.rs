use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::{collections::{BTreeMap, HashMap}, fs, net::IpAddr, path::Path};

const HEADER: &str =
    "# Hisoka configuration\n# Restart the service after editing: hisoka restart\n\n";

/// Windows always queries DNS servers on port 53.
pub const DNS_PORT: u16 = 53;
/// Seconds a client may cache an answer.
pub const TTL_SECS: u32 = 30;

/// Port the resolver binds to. Dev-only override: set HISOKA_PORT=5353
/// to avoid clashing with something already using 53.
pub fn dns_port() -> u16 {
    std::env::var("HISOKA_PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DNS_PORT)
}

pub type Rules = BTreeMap<String, Rule>;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub log: LogConfig,
    /// Wildcard rules by id.
    pub wildcard: Rules,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LogConfig {
    /// Delete log files older than this many days.
    pub retention_days: u32,
    /// Delete the oldest logs when the folder exceeds this size.
    pub max_mb: u32,
    /// Log every DNS query (noisy, keep off).
    pub debug: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// "*.example.test" (wildcard) or "api.example.test" (exact).
    pub pattern: String,
    pub address: IpAddr,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self { retention_days: 7, max_mb: 10, debug: false }
    }
}

// ---------- pattern + id rules ----------

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

/// Lowercase, strip a trailing dot, and check the grammar:
/// either an exact name, or `*.` followed by an exact name.
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

// ---------- matching ----------

/// Built once from a validated config; `find` is the per-query lookup.
pub struct Matcher {
    exact: HashMap<String, (String, Rule)>,
    /// (suffix with leading dot, id, rule), longest suffix first.
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

    /// Exact name first, then the longest matching wildcard suffix.
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

    /// Read the file; returns defaults if it doesn't exist. Never writes.
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

    // ----- rule editing (each validates the whole config, rolls back on error) -----

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

    /// Set a scalar by dotted key, e.g. "log.retention_days" = "14".
    /// Rules are edited with add_rule / set_rule / remove_rule instead.
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