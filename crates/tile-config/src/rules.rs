use regex::{Regex, RegexBuilder};
use tile_core::WindowInfo;

use crate::{ConfigError, Rule};

#[derive(Debug, Clone)]
struct CompiledRule {
    exe: Option<Regex>,
    class: Option<Regex>,
    title: Option<Regex>,
    rule: Rule,
}

impl CompiledRule {
    fn matches(&self, info: &WindowInfo) -> bool {
        let check =
            |re: &Option<Regex>, value: &str| re.as_ref().is_none_or(|re| re.is_match(value));
        check(&self.exe, &info.exe)
            && check(&self.class, &info.class)
            && check(&self.title, &info.title)
    }
}

/// Compiled window rules, applied in order (later rules override earlier ones).
#[derive(Debug, Clone, Default)]
pub struct Rules(Vec<CompiledRule>);

/// The combined effect of every matching rule.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RuleOutcome {
    /// Leave the window alone.
    pub ignore: bool,
    /// Float (or tile) it, whatever the layout.
    pub float: Option<bool>,
    /// Open it on this workspace.
    pub workspace: Option<u8>,
    /// Open it on this monitor, by index.
    pub monitor: Option<usize>,
    /// Force its title bar on or off.
    pub decorations: Option<bool>,
}

impl Rules {
    /// Compiles the rules' patterns, naming the first rule with an invalid one.
    pub fn compile(rules: &[Rule]) -> Result<Self, ConfigError> {
        let compile = |i: usize, pattern: &Option<String>| {
            pattern
                .as_deref()
                .map(|p| RegexBuilder::new(p).case_insensitive(true).build())
                .transpose()
                .map_err(|e| ConfigError::Rule(i + 1, e.to_string()))
        };
        rules
            .iter()
            .enumerate()
            .map(|(i, rule)| {
                if rule.exe.is_none() && rule.class.is_none() && rule.title.is_none() {
                    return Err(ConfigError::Rule(
                        i + 1,
                        "needs at least one of exe, class, title".into(),
                    ));
                }
                Ok(CompiledRule {
                    exe: compile(i, &rule.exe)?,
                    class: compile(i, &rule.class)?,
                    title: compile(i, &rule.title)?,
                    rule: rule.clone(),
                })
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }

    /// What the rules matching `info` add up to, later rules winning.
    pub fn resolve(&self, info: &WindowInfo) -> RuleOutcome {
        let mut out = RuleOutcome::default();
        for compiled in self.0.iter().filter(|r| r.matches(info)) {
            let rule = &compiled.rule;
            out.ignore |= rule.ignore;
            out.float = rule.float.or(out.float);
            out.workspace = rule.workspace.or(out.workspace);
            out.monitor = rule.monitor.or(out.monitor);
            out.decorations = rule.decorations.or(out.decorations);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(exe: &str, class: &str, title: &str) -> WindowInfo {
        WindowInfo {
            exe: exe.into(),
            class: class.into(),
            title: title.into(),
        }
    }

    #[test]
    fn matches_all_given_fields() {
        let rules = Rules::compile(&[
            Rule {
                exe: Some("^spotify\\.exe$".into()),
                workspace: Some(9),
                ..Rule::default()
            },
            Rule {
                exe: Some("firefox".into()),
                title: Some("Picture-in-Picture".into()),
                float: Some(true),
                ..Rule::default()
            },
        ])
        .unwrap();
        assert_eq!(
            rules.resolve(&info("Spotify.exe", "x", "y")).workspace,
            Some(9)
        );
        let decor = Rules::compile(&[Rule {
            class: Some("^Notepad$".into()),
            decorations: Some(true),
            ..Rule::default()
        }])
        .unwrap();
        assert_eq!(
            decor
                .resolve(&info("notepad.exe", "Notepad", "x"))
                .decorations,
            Some(true)
        );
        assert_eq!(
            rules
                .resolve(&info("firefox.exe", "x", "Picture-in-Picture"))
                .float,
            Some(true)
        );
        assert_eq!(
            rules.resolve(&info("firefox.exe", "x", "Mozilla")),
            RuleOutcome::default()
        );
    }

    #[test]
    fn rejects_bad_rules() {
        assert!(
            Rules::compile(&[Rule {
                float: Some(true),
                ..Rule::default()
            }])
            .is_err()
        );
        assert!(
            Rules::compile(&[Rule {
                exe: Some("(".into()),
                ..Rule::default()
            }])
            .is_err()
        );
    }
}
