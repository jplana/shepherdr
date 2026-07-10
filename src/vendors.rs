use crate::config::{Config, VendorSpec};
use anyhow::{anyhow, Result};
use std::collections::HashSet;

pub fn resolve<'a>(
    role: &str,
    config: &'a Config,
    vendor: Option<&str>,
    unavailable: &HashSet<String>,
) -> Result<(&'a str, &'a VendorSpec)> {
    if let Some(v) = vendor {
        let (key, spec) = config.vendors.get_key_value(v)
            .ok_or_else(|| anyhow!("unknown vendor: {v}"))?;
        return Ok((key.as_str(), spec));
    }
    let chain = config.roles.get(role).ok_or_else(|| anyhow!("unknown role: {role}"))?;
    for candidate in chain {
        if !unavailable.contains(candidate) {
            let (k, spec) = config.vendors.get_key_value(candidate)
                .ok_or_else(|| anyhow!("chain names unknown vendor: {candidate}"))?;
            return Ok((k.as_str(), spec));
        }
    }
    Err(anyhow!("all vendors exhausted for role {role}: {chain:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::default_config;
    use std::collections::HashSet;

    fn none() -> HashSet<String> { HashSet::new() }

    #[test]
    fn resolves_first_in_chain() {
        let c = default_config();
        let (v, spec) = resolve("implementer", &c, None, &none()).unwrap();
        assert_eq!(v, "claude");
        assert!(spec.command.contains("claude -p"));
    }

    #[test]
    fn falls_back_when_preferred_unavailable() {
        let c = default_config();
        let un: HashSet<String> = ["codex".to_string()].into_iter().collect();
        let (v, _) = resolve("reviewer", &c, None, &un).unwrap();
        assert_eq!(v, "cursor");
    }

    #[test]
    fn explicit_override_wins() {
        let c = default_config();
        let (v, _) = resolve("implementer", &c, Some("codex"), &none()).unwrap();
        assert_eq!(v, "codex");
    }

    #[test]
    fn unknown_role_errors() {
        let c = default_config();
        assert!(resolve("nope", &c, None, &none()).is_err());
    }

    #[test]
    fn exhausted_chain_errors() {
        let c = default_config();
        let un: HashSet<String> = ["codex", "cursor"].iter().map(|s| s.to_string()).collect();
        assert!(resolve("reviewer", &c, None, &un).is_err());
    }
}
