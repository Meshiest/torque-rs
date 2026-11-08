//! The cargo features a bundle is built with: the ones `bundle.toml` always wants, and the ones `game.toml` gives by which
//! engine plugins it keeps on.
//!
//! An engine plugin whose code is behind a cargo feature is in [`PLUGIN_FEATURES`]. A game crate that forwards that feature
//! (`gizmos = ["torque/gizmos"]`) gets it when the plugin is on: a plugin is on unless `[plugins] disable` names it, and a
//! debug plugin (one a shipped game leaves out) is on only when `[plugins] enable` names it. The build turns the crate's
//! default features off, so what is not derived or asked for is not compiled in.

use std::collections::BTreeSet;

use serde::Deserialize;

/// An engine plugin carried by a cargo feature.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginFeature {
    /// The plugin's id, as `game.toml` names it.
    pub plugin: &'static str,
    /// The feature a game crate forwards to the engine's.
    pub feature: &'static str,
    /// Off in a bundle unless `[plugins] enable` names it.
    pub debug: bool,
}

/// The engine's plugins that cargo features carry.
pub const PLUGIN_FEATURES: &[PluginFeature] = &[
    PluginFeature {
        plugin: "torque.gizmos",
        feature: "gizmos",
        debug: true,
    },
    PluginFeature {
        plugin: "torque.audio",
        feature: "audio-device",
        debug: false,
    },
];

/// `game.toml`'s `[plugins]` lists, the only part of it packaging reads (the engine reads the rest when the game runs).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Toggles {
    pub disable: Vec<String>,
    pub enable: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawConfig {
    plugins: RawPlugins,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawPlugins {
    disable: Vec<String>,
    enable: Vec<String>,
}

impl Toggles {
    /// The toggles in `game.toml`'s text. Keys other than `[plugins]` are the engine's to check.
    pub fn parse(text: &str) -> Result<Toggles, String> {
        let raw: RawConfig =
            toml::from_str(text).map_err(|e| format!("game.toml: {}", e.message()))?;
        Ok(Toggles {
            disable: raw.plugins.disable,
            enable: raw.plugins.enable,
        })
    }

    /// Whether the plugin `p` is in the bundle. `enable` wins over `disable`, as when the game runs.
    pub fn is_on(&self, p: &PluginFeature) -> bool {
        let named = |list: &[String]| list.iter().any(|id| id == p.plugin);
        if named(&self.enable) {
            return true;
        }
        !p.debug && !named(&self.disable)
    }
}

/// The features to build with: `always`, and each feature of [`PLUGIN_FEATURES`] that the crate has (`crate_features`) and whose
/// plugin is on. Sorted, so the same inputs give the same command line.
pub fn derive(
    toggles: &Toggles,
    crate_features: &BTreeSet<String>,
    always: &[String],
) -> Vec<String> {
    let mut features: BTreeSet<String> = always.iter().cloned().collect();
    for p in PLUGIN_FEATURES {
        if crate_features.contains(p.feature) && toggles.is_on(p) {
            features.insert(p.feature.to_string());
        }
    }
    features.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crate_features(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn debug_plugins_are_left_out_unless_enabled() {
        let has = crate_features(&["gizmos", "bundled"]);
        let always = ["bundled".to_string()];
        let none = Toggles::default();
        assert_eq!(derive(&none, &has, &always), ["bundled"]);
        let on = Toggles::parse("[plugins]\nenable = [\"torque.gizmos\"]\n").unwrap();
        assert_eq!(derive(&on, &has, &always), ["bundled", "gizmos"]);
    }

    #[test]
    fn a_disabled_plugin_drops_its_feature_and_enable_wins() {
        let has = crate_features(&["audio-device"]);
        let off = Toggles::parse("[plugins]\ndisable = [\"torque.audio\"]\n").unwrap();
        assert!(derive(&off, &has, &[]).is_empty());
        assert_eq!(derive(&Toggles::default(), &has, &[]), ["audio-device"]);
        let both = Toggles::parse(
            "[plugins]\ndisable = [\"torque.audio\"]\nenable = [\"torque.audio\"]\n",
        )
        .unwrap();
        assert_eq!(derive(&both, &has, &[]), ["audio-device"]);
    }

    #[test]
    fn a_feature_the_crate_does_not_forward_is_never_passed() {
        let on = Toggles::parse("[plugins]\nenable = [\"torque.gizmos\"]\n").unwrap();
        assert!(derive(&on, &BTreeSet::new(), &[]).is_empty());
    }

    #[test]
    fn the_rest_of_game_toml_is_not_read_here() {
        let toggles = Toggles::parse(
            "[game]\nname = \"x\"\n[runner]\nid = \"torque.dedicated-runner\"\n[mods]\nenabled = false\n",
        )
        .unwrap();
        assert_eq!(toggles, Toggles::default());
        assert!(Toggles::parse("[plugins]\ndisable = 3\n").is_err());
    }
}
