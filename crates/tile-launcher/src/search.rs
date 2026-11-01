//! Searching the app launcher's list: fuzzy matching, ranking and launch history.
//!
//! The matcher is a port of [fzy](https://github.com/jhawthorn/fzy)'s scoring, and the
//! weights follow Noctalia's launcher: the app name always outranks the id, a name that
//! starts with the query gets a bonus, and launch counts nudge close matches.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use tile_core::{Command, command::DISPATCHERS};

/// An app the launcher can start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct App {
    /// What Start shows, like `Windows Terminal`.
    pub name: String,
    /// The `shell:AppsFolder` id: an AppUserModelID or a path.
    pub id: String,
}

const GAP_LEADING: f64 = -0.005;
const GAP_TRAILING: f64 = -0.005;
const GAP_INNER: f64 = -0.01;
const MATCH_CONSECUTIVE: f64 = 1.0;
const MATCH_SLASH: f64 = 0.9;
const MATCH_WORD: f64 = 0.8;
const MATCH_CAPITAL: f64 = 0.7;
const MATCH_DOT: f64 = 0.6;
/// Score for an exact (case-insensitive) match.
const EXACT: f64 = 1024.0;
/// Longer haystacks still match but aren't scored, like fzy.
const MAX_LEN: usize = 1024;

const NAME_WEIGHT: f64 = 5.0;
const NAME_PREFIX_BONUS: f64 = 500.0;
const NAME_PRIORITY: f64 = 3000.0;
const ID_WEIGHT: f64 = 1.5;
const USAGE_PER_COUNT: f64 = 0.1;
const TYPED_USAGE_CAP: f64 = 0.5;
/// How many recent launches are remembered.
const RECENT_LEN: usize = 20;

fn bonus(last: char, ch: char) -> f64 {
    let separator = match last {
        '/' | '\\' => MATCH_SLASH,
        '-' | '_' | ' ' => MATCH_WORD,
        '.' => MATCH_DOT,
        _ => 0.0,
    };
    if ch.is_uppercase() {
        if last.is_lowercase() {
            MATCH_CAPITAL
        } else {
            separator
        }
    } else if ch.is_alphanumeric() {
        separator
    } else {
        0.0
    }
}

fn lower(c: char) -> char {
    c.to_lowercase().next().unwrap_or(c)
}

/// How well `needle` fuzzy-matches `haystack`, ignoring case, or `None` if its characters
/// don't all appear in order. Higher is better.
pub fn fuzzy(needle: &str, haystack: &str) -> Option<f64> {
    let needle: Vec<char> = needle.chars().map(lower).collect();
    let original: Vec<char> = haystack.chars().collect();
    let hay: Vec<char> = original.iter().copied().map(lower).collect();
    if needle.is_empty() {
        return Some(0.0);
    }
    let mut rest = hay.iter();
    if !needle.iter().all(|n| rest.any(|h| h == n)) {
        return None;
    }
    let (n, m) = (needle.len(), hay.len());
    if n == m {
        return Some(EXACT);
    }
    if m > MAX_LEN {
        return Some(f64::MIN);
    }
    let mut last = '/';
    let bonuses: Vec<f64> = original
        .iter()
        .map(|&ch| {
            let b = bonus(last, ch);
            last = ch;
            b
        })
        .collect();

    // d: best score ending in a match here; mm: best score up to here.
    let mut d = vec![f64::NEG_INFINITY; m];
    let mut mm = vec![f64::NEG_INFINITY; m];
    for (i, &nc) in needle.iter().enumerate() {
        let gap = if i == n - 1 { GAP_TRAILING } else { GAP_INNER };
        let mut prev_score = f64::NEG_INFINITY;
        let (mut prev_d, mut prev_m) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        for j in 0..m {
            if nc == hay[j] {
                let score = if i == 0 {
                    j as f64 * GAP_LEADING + bonuses[j]
                } else if j > 0 {
                    (prev_m + bonuses[j]).max(prev_d + MATCH_CONSECUTIVE)
                } else {
                    f64::NEG_INFINITY
                };
                prev_d = d[j];
                prev_m = mm[j];
                d[j] = score;
                prev_score = score.max(prev_score + gap);
            } else {
                prev_d = d[j];
                prev_m = mm[j];
                d[j] = f64::NEG_INFINITY;
                prev_score += gap;
            }
            mm[j] = prev_score;
        }
    }
    Some(mm[m - 1])
}

/// The part of an id worth matching: the file name of a path, or the app name of an
/// AppUserModelID (`Microsoft.WindowsTerminal_8wekyb3d8bbwe!App` gives `WindowsTerminal`).
fn id_stem(id: &str) -> &str {
    let file = id.rsplit(['\\', '/']).next().unwrap_or(id);
    let file = match file.split_once('!') {
        // The package family name ends in `_` and the publisher hash.
        Some((family, _)) => family.split('_').next().unwrap_or(family),
        None => file,
    };
    let file = file.rsplit_once('.').map_or(file, |(head, tail)| {
        if tail.eq_ignore_ascii_case("exe") || tail.eq_ignore_ascii_case("lnk") {
            head
        } else {
            tail
        }
    });
    if file.is_empty() { id } else { file }
}

/// How well `query` matches `app`, or `None` if it doesn't.
pub fn score(query: &str, app: &App) -> Option<f64> {
    if query.is_empty() {
        return Some(0.0);
    }
    if let Some(s) = fuzzy(query, &app.name) {
        let prefix = app.name.to_lowercase().starts_with(&query.to_lowercase());
        let bonus = if prefix { NAME_PREFIX_BONUS } else { 0.0 };
        return Some(s * NAME_WEIGHT + bonus + NAME_PRIORITY);
    }
    fuzzy(query, id_stem(&app.id)).map(|s| s * ID_WEIGHT)
}

/// Launch history, saved between runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Usage {
    /// Launches per app id.
    pub counts: BTreeMap<String, u32>,
    /// App ids, most recently launched first.
    pub recent: Vec<String>,
}

impl Usage {
    /// Notes a launch of the app with this id.
    pub fn record(&mut self, id: &str) {
        *self.counts.entry(id.to_owned()).or_default() += 1;
        self.recent.retain(|r| r != id);
        self.recent.insert(0, id.to_owned());
        self.recent.truncate(RECENT_LEN);
    }

    fn count(&self, id: &str) -> u32 {
        self.counts.get(id).copied().unwrap_or(0)
    }

    fn recency(&self, id: &str) -> Option<usize> {
        self.recent.iter().position(|r| r == id)
    }
}

/// How [`rank`] orders results.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RankOptions {
    /// Names or ids listed first, in this order, when the query is empty.
    pub pinned: Vec<String>,
    /// Let launch counts and recent launches affect the order.
    pub sort_by_usage: bool,
    /// The most results returned.
    pub max_results: usize,
}

fn pin_index(pinned: &[String], app: &App) -> Option<usize> {
    pinned
        .iter()
        .position(|p| p.eq_ignore_ascii_case(&app.name) || p.eq_ignore_ascii_case(&app.id))
}

/// The indices into `apps` to show for `query`, best first.
///
/// With no query: pinned apps, then (with `sort_by_usage`) recent launches, then the rest by
/// name. With a query: matches by score, launch counts breaking near ties.
pub fn rank(query: &str, apps: &[App], usage: &Usage, options: &RankOptions) -> Vec<usize> {
    let query = query.trim();
    let mut out: Vec<usize> = if query.is_empty() {
        let mut order: Vec<usize> = (0..apps.len()).collect();
        order.sort_by_cached_key(|&i| {
            let app = &apps[i];
            let pin = pin_index(&options.pinned, app).unwrap_or(usize::MAX);
            let recent = if options.sort_by_usage {
                usage.recency(&app.id).unwrap_or(usize::MAX)
            } else {
                usize::MAX
            };
            (pin, recent, app.name.to_lowercase())
        });
        order
    } else {
        let mut scored: Vec<(f64, usize)> = apps
            .iter()
            .enumerate()
            .filter_map(|(i, app)| {
                let mut s = score(query, app)?;
                if options.sort_by_usage {
                    let boost = f64::from(usage.count(&app.id)) * USAGE_PER_COUNT;
                    s += boost.min(TYPED_USAGE_CAP);
                }
                Some((s, i))
            })
            .collect();
        scored.sort_by(|a, b| {
            b.0.total_cmp(&a.0)
                .then_with(|| apps[a.1].name.len().cmp(&apps[b.1].name.len()))
                .then_with(|| apps[a.1].name.cmp(&apps[b.1].name))
        });
        scored.into_iter().map(|(_, i)| i).collect()
    };
    out.truncate(options.max_results);
    out
}

/// A command row of the launcher's results.
#[derive(Debug, Clone, PartialEq)]
pub enum CommandEntry {
    /// Run a Tile dispatcher.
    Dispatch(Command),
    /// A dispatcher that takes an argument: picking it completes its name in the search box.
    Complete {
        name: &'static str,
        args: &'static str,
    },
    /// Run a dispatcher that takes no argument.
    Name(&'static str),
    /// A known dispatcher whose arguments don't parse yet: shows its usage and the error.
    Invalid {
        name: &'static str,
        args: &'static str,
        error: String,
    },
    /// Run the text as a command line, like the Run dialog.
    Shell(String),
}

/// The rows for command-mode text `text`: a dispatcher it parses as, then dispatchers whose
/// name starts with its first word (or its usage if the arguments are incomplete), then running
/// it as a command line.
pub fn commands(text: &str) -> Vec<CommandEntry> {
    let text = text.trim();
    if text.is_empty() {
        return DISPATCHERS.iter().map(|&(n, a)| entry(n, a)).collect();
    }
    let mut out = Vec::new();
    let word = text
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .to_lowercase();
    match text.parse::<Command>() {
        Ok(command) => out.push(CommandEntry::Dispatch(command)),
        Err(error) => {
            if let Some(&(name, args)) = DISPATCHERS.iter().find(|(n, _)| *n == word) {
                out.push(CommandEntry::Invalid {
                    name,
                    args,
                    error: error.to_string(),
                });
            } else if !text.contains(char::is_whitespace) {
                out.extend(
                    DISPATCHERS
                        .iter()
                        .filter(|(n, _)| n.starts_with(&word))
                        .map(|&(n, a)| entry(n, a)),
                );
            }
        }
    }
    out.push(CommandEntry::Shell(text.to_owned()));
    out
}

fn entry(name: &'static str, args: &'static str) -> CommandEntry {
    if args.is_empty() {
        CommandEntry::Name(name)
    } else {
        CommandEntry::Complete { name, args }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(name: &str, id: &str) -> App {
        App {
            name: name.into(),
            id: id.into(),
        }
    }

    fn names(query: &str, apps: &[App], usage: &Usage, options: &RankOptions) -> Vec<String> {
        rank(query, apps, usage, options)
            .into_iter()
            .map(|i| apps[i].name.clone())
            .collect()
    }

    fn options() -> RankOptions {
        RankOptions {
            pinned: Vec::new(),
            sort_by_usage: true,
            max_results: 50,
        }
    }

    #[test]
    fn fuzzy_needs_every_character_in_order() {
        assert!(fuzzy("trm", "Terminal").is_some());
        assert!(fuzzy("mrt", "Terminal").is_none());
        assert_eq!(fuzzy("TERMINAL", "terminal"), Some(EXACT));
    }

    #[test]
    fn word_starts_and_runs_beat_scattered_letters() {
        let start = fuzzy("vs", "Visual Studio").unwrap();
        let scattered = fuzzy("vs", "Velvets").unwrap();
        assert!(start > scattered);
        let run = fuzzy("term", "Terminal").unwrap();
        let spread = fuzzy("term", "The Remote Manager").unwrap();
        assert!(run > spread);
    }

    #[test]
    fn name_prefix_wins_and_names_beat_ids() {
        let apps = [
            app("Paint", "Microsoft.Paint_8wekyb3d8bbwe!App"),
            app("Snipping Tool", "Microsoft.ScreenSketch_8wekyb3d8bbwe!App"),
            app("Notepad", "Microsoft.WindowsNotepad_8wekyb3d8bbwe!App"),
            app("Tool", "C:\\x\\paintbrush.exe"),
        ];
        assert_eq!(
            names("pa", &apps, &Usage::default(), &options()),
            ["Paint", "Notepad", "Tool"]
        );
    }

    #[test]
    fn ids_match_by_their_stem() {
        assert_eq!(
            id_stem("Microsoft.WindowsTerminal_8wekyb3d8bbwe!App"),
            "WindowsTerminal"
        );
        assert_eq!(id_stem("C:\\Program Files\\Foo\\bar.exe"), "bar");
        assert_eq!(
            id_stem("{6D809377-6AF0-444B-8957-A3773F02200E}\\x\\code.exe"),
            "code"
        );
    }

    #[test]
    fn usage_breaks_ties_but_not_strong_matches() {
        let apps = [app("Code A", "a"), app("Code B", "b"), app("Calendar", "c")];
        let mut usage = Usage::default();
        for _ in 0..100 {
            usage.record("b");
            usage.record("c");
        }
        assert_eq!(
            names("code", &apps, &usage, &options()),
            ["Code B", "Code A"]
        );
        let plain = RankOptions {
            sort_by_usage: false,
            ..options()
        };
        assert_eq!(names("code", &apps, &usage, &plain), ["Code A", "Code B"]);
    }

    #[test]
    fn empty_query_lists_pinned_then_recent_then_names() {
        let apps = [app("b", "b"), app("a", "a"), app("c", "c"), app("d", "d")];
        let mut usage = Usage::default();
        usage.record("c");
        let options = RankOptions {
            pinned: vec!["D".into()],
            ..options()
        };
        assert_eq!(names("", &apps, &usage, &options), ["d", "c", "a", "b"]);
    }

    #[test]
    fn recent_is_capped_and_deduplicated() {
        let mut usage = Usage::default();
        for i in 0..30 {
            usage.record(&i.to_string());
        }
        usage.record("25");
        assert_eq!(usage.recent.len(), RECENT_LEN);
        assert_eq!(usage.recent[0], "25");
        assert_eq!(usage.recent.iter().filter(|r| *r == "25").count(), 1);
    }

    #[test]
    fn known_dispatchers_with_bad_arguments_show_their_usage() {
        for text in ["layout", "layout mas"] {
            let rows = commands(text);
            assert!(matches!(
                rows[0],
                CommandEntry::Invalid { name: "layout", .. }
            ));
            assert_eq!(rows[1], CommandEntry::Shell(text.into()));
        }
    }

    #[test]
    fn commands_offer_dispatchers_then_the_shell() {
        assert_eq!(
            commands(" workspace 3 "),
            [
                CommandEntry::Dispatch(Command::Workspace(3)),
                CommandEntry::Shell("workspace 3".into()),
            ]
        );
        assert_eq!(
            commands("work"),
            [
                CommandEntry::Complete {
                    name: "workspace",
                    args: "<1-255>"
                },
                CommandEntry::Shell("work".into()),
            ]
        );
        assert_eq!(
            commands("notepad foo.txt"),
            [CommandEntry::Shell("notepad foo.txt".into())]
        );
        assert_eq!(commands("").len(), DISPATCHERS.len());
    }

    #[test]
    fn results_are_capped() {
        let apps: Vec<App> = (0..10).map(|i| app(&format!("app {i}"), "x")).collect();
        let options = RankOptions {
            max_results: 3,
            ..options()
        };
        assert_eq!(rank("app", &apps, &Usage::default(), &options).len(), 3);
    }
}
