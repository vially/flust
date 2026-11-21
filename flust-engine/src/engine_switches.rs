//! Engine switches passed through the environment by the `flutter` tool.
//!
//! `flutter run` turns its options (`--start-paused`, `--dart-flags`,
//! `--trace-startup`, `--enable-impeller`, …) into engine switches. Flutter's
//! own Linux and Windows shells read them from `FLUTTER_ENGINE_SWITCHES` (the
//! count) and `FLUTTER_ENGINE_SWITCH_1` … `FLUTTER_ENGINE_SWITCH_<n>`, each a
//! switch without its leading `--` (`shell/platform/common/engine_switches.cc`
//! in the engine). Flust's tool extension passes them the same way.

use tracing::warn;

/// At most this many switches are read, as in Flutter's shells.
const MAX_SWITCHES: usize = 50;

/// The engine switches in the process environment, as command-line arguments
/// (`--name=value`).
///
/// Release builds ignore them, as Flutter's shells do: a deployed app
/// shouldn't take debugging switches from its environment.
pub(crate) fn from_env() -> Vec<String> {
    if flust_engine_sys::ENGINE_BUILD_MODE == "release" {
        return Vec::new();
    }
    parse(|name| std::env::var(name).ok())
}

fn parse(lookup: impl Fn(&str) -> Option<String>) -> Vec<String> {
    let Some(count) = lookup("FLUTTER_ENGINE_SWITCHES") else {
        return Vec::new();
    };
    let Ok(count) = count.trim().parse::<usize>() else {
        warn!("ignoring FLUTTER_ENGINE_SWITCHES={count}: not a number");
        return Vec::new();
    };
    if count > MAX_SWITCHES {
        warn!("FLUTTER_ENGINE_SWITCHES={count}: only the first {MAX_SWITCHES} switches are used");
    }
    (1..=count.min(MAX_SWITCHES))
        .filter_map(|index| {
            let name = format!("FLUTTER_ENGINE_SWITCH_{index}");
            match lookup(&name) {
                Some(switch) => Some(format!("--{switch}")),
                None => {
                    warn!("FLUTTER_ENGINE_SWITCHES={count}, but {name} is not set");
                    None
                }
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::parse;
    use std::collections::HashMap;

    fn parse_env(vars: &[(&str, &str)]) -> Vec<String> {
        let vars: HashMap<String, String> = vars
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        parse(|name| vars.get(name).cloned())
    }

    #[test]
    fn no_switches() {
        assert!(parse_env(&[]).is_empty());
        assert!(parse_env(&[("FLUTTER_ENGINE_SWITCH_1", "start-paused=true")]).is_empty());
    }

    #[test]
    fn switches_in_order() {
        assert_eq!(
            parse_env(&[
                ("FLUTTER_ENGINE_SWITCHES", "2"),
                ("FLUTTER_ENGINE_SWITCH_2", "dart-flags=--foo"),
                ("FLUTTER_ENGINE_SWITCH_1", "start-paused=true"),
            ]),
            ["--start-paused=true", "--dart-flags=--foo"]
        );
    }

    #[test]
    fn missing_switch_is_skipped() {
        assert_eq!(
            parse_env(&[
                ("FLUTTER_ENGINE_SWITCHES", "2"),
                ("FLUTTER_ENGINE_SWITCH_2", "verbose-logging=true"),
            ]),
            ["--verbose-logging=true"]
        );
    }

    #[test]
    fn invalid_count() {
        assert!(parse_env(&[("FLUTTER_ENGINE_SWITCHES", "two")]).is_empty());
    }

    #[test]
    fn count_is_capped() {
        let mut vars = vec![("FLUTTER_ENGINE_SWITCHES".to_string(), "60".to_string())];
        for index in 1..=60 {
            vars.push((
                format!("FLUTTER_ENGINE_SWITCH_{index}"),
                format!("s{index}"),
            ));
        }
        let vars: Vec<(&str, &str)> = vars.iter().map(|(n, v)| (n.as_str(), v.as_str())).collect();
        assert_eq!(parse_env(&vars).len(), 50);
    }
}
