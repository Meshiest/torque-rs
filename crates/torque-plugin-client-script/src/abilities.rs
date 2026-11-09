//! The abilities a player decides on: the tiers a server may ask for, each with a stable key and the English line the default
//! prompt shows. A game that translates its UI looks the key up in its own string table and falls back to the English text.

use torque_client_script_format::Tier;

/// The tiers a server may ask for and a player decide on (every tier but the base, which every realm has), in the order a
/// prompt lists them.
pub const ABILITIES: [Tier; 9] = [
    Tier::Ui,
    Tier::Net,
    Tier::State,
    Tier::Draw,
    Tier::Effects,
    Tier::Input,
    Tier::Camera,
    Tier::Local,
    Tier::Predict,
];

/// The prefix of every ability key.
pub const KEY_PREFIX: &str = "client_script.ability.";

/// The stable key of an ability's line: `client_script.ability.<tier name>`.
pub fn key(tier: Tier) -> String {
    format!("{KEY_PREFIX}{}", tier.name())
}

/// The ability a key names, if it is one of [`ABILITIES`].
pub fn from_key(key: &str) -> Option<Tier> {
    let tier = Tier::from_name(key.strip_prefix(KEY_PREFIX)?)?;
    ABILITIES.contains(&tier).then_some(tier)
}

/// The English line for an ability, in player words.
pub fn english(tier: Tier) -> &'static str {
    match tier {
        Tier::Ui => "Show its own menus and HUD",
        Tier::Net => "Send messages to its own server",
        Tier::State => "See the players and objects your game already shows",
        Tier::Draw => "Draw markers and lines in the world",
        Tier::Effects => "Play sounds and effects",
        Tier::Input => "Add its own keys to your controls",
        Tier::Camera => "Move your camera for short scenes",
        Tier::Local => "Show objects only you can see",
        Tier::Predict => "Use its own rules to smooth your player's movement",
        Tier::Realm => "",
    }
}

/// Whether an ability's box starts ticked: every reviewed one does, because the review is what makes it safe, except the
/// camera when the player asked for reduced motion.
pub fn default_on(tier: Tier, reduce_motion: bool) -> bool {
    !(tier == Tier::Camera && reduce_motion)
}

/// The key of the line a game shows beside the option that grants the basic abilities without asking.
pub const BASICS_KEY: &str = "client_script.basics_without_asking";

/// The English line for that option. It names what [`is_basic`] covers, which is more than menus: a script may also message
/// its own server and read the game state the player's client already holds.
pub const BASICS_ENGLISH: &str = "Allow without asking: menus, HUD, markers and sounds. These scripts can also send messages to their own server and read the players and objects your game already shows.";

/// Whether an ability is one of the basic ones the "allow without asking" option covers: `ui` (menus and HUD), `net` (messages
/// to the script's own server), `state` (read-only views of replicated game state), `draw` (markers and lines) and `effects`
/// (sounds and effects).
pub fn is_basic(tier: Tier) -> bool {
    matches!(
        tier,
        Tier::Ui | Tier::Net | Tier::State | Tier::Draw | Tier::Effects
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_basics_line_names_everything_the_option_grants() {
        let covered: Vec<Tier> = ABILITIES.into_iter().filter(|t| is_basic(*t)).collect();
        assert_eq!(
            covered,
            [Tier::Ui, Tier::Net, Tier::State, Tier::Draw, Tier::Effects]
        );
        for word in [
            "menus", "HUD", "markers", "sounds", "messages", "server", "read",
        ] {
            assert!(BASICS_ENGLISH.contains(word), "{word}");
        }
        assert!(english(Tier::Net).contains("server"));
    }
}
