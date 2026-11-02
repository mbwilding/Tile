//! Compiled `notifications.filter` entries: the same regex-on-a-name matching as [`Rule`], but
//! against a posted notification's app name instead of a window.

use regex::{Regex, RegexBuilder};

use crate::{ConfigError, NotificationFilter};

#[derive(Debug, Clone)]
struct Compiled {
    app: Option<Regex>,
    filter: NotificationFilter,
}

impl Compiled {
    fn matches(&self, app: &str) -> bool {
        self.app.as_ref().is_none_or(|re| re.is_match(app))
    }
}

/// Compiled `notifications.filter` entries, applied in order (later entries override earlier
/// ones for the fields they set).
#[derive(Debug, Clone, Default)]
pub struct NotificationFilters(Vec<Compiled>);

/// What the filters matching a notification's app add up to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FilterOutcome {
    /// Drop the notification instead of showing a popup for it.
    pub mute: bool,
    /// Overrides `notifications.duration_ms`.
    pub duration_ms: Option<u64>,
}

impl NotificationFilters {
    /// Compiles the filters' patterns, naming the first one with an invalid one.
    pub fn compile(filters: &[NotificationFilter]) -> Result<Self, ConfigError> {
        filters
            .iter()
            .enumerate()
            .map(|(i, filter)| {
                let app = filter
                    .app
                    .as_deref()
                    .map(|p| RegexBuilder::new(p).case_insensitive(true).build())
                    .transpose()
                    .map_err(|e| ConfigError::NotificationFilter(i + 1, e.to_string()))?;
                Ok(Compiled {
                    app,
                    filter: filter.clone(),
                })
            })
            .collect::<Result<_, _>>()
            .map(Self)
    }

    /// What the filters matching `app` add up to, later filters winning.
    pub fn resolve(&self, app: &str) -> FilterOutcome {
        let mut out = FilterOutcome::default();
        for compiled in self.0.iter().filter(|c| c.matches(app)) {
            out.mute |= compiled.filter.mute;
            out.duration_ms = compiled.filter.duration_ms.or(out.duration_ms);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutes_and_overrides_by_app_name() {
        let filters = NotificationFilters::compile(&[
            NotificationFilter {
                app: Some("^Spotify$".into()),
                duration_ms: Some(1000),
                ..NotificationFilter::default()
            },
            NotificationFilter {
                app: Some("Teams".into()),
                mute: true,
                ..NotificationFilter::default()
            },
        ])
        .unwrap();
        assert_eq!(filters.resolve("Spotify").duration_ms, Some(1000));
        assert!(!filters.resolve("Spotify").mute);
        assert!(filters.resolve("Microsoft Teams").mute);
        assert_eq!(filters.resolve("Mail"), FilterOutcome::default());
    }

    #[test]
    fn rejects_a_bad_pattern() {
        assert!(
            NotificationFilters::compile(&[NotificationFilter {
                app: Some("(".into()),
                ..NotificationFilter::default()
            }])
            .is_err()
        );
    }
}
