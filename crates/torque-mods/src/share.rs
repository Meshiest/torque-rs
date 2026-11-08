//! What a server asks of clients for each of its mods: [`Share`], and the slot that decides it ([`ModShare`]).

use crate::content::ContentSummary;
use crate::info::ModInfo;

/// What a server asks of clients for one of its mods.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Share {
    /// Nothing: clients need not have the mod (a server-only mod).
    None,
    /// Clients must have the mod's content, at the server's version, to join. The server does not send it.
    Required,
    /// As [`Required`](Share::Required), and the server sends the content to a client that lacks it (when the server lets
    /// clients download, and the content fits the caps).
    Download,
}

impl Share {
    /// The word `mod.toml` writes it as: `none`, `required` or `download`.
    pub fn parse(text: &str) -> Option<Share> {
        match text.to_ascii_lowercase().as_str() {
            "none" => Some(Share::None),
            "required" => Some(Share::Required),
            "download" => Some(Share::Download),
            _ => None,
        }
    }

    /// The word for the share.
    pub fn name(self) -> &'static str {
        match self {
            Share::None => "none",
            Share::Required => "required",
            Share::Download => "download",
        }
    }
}

/// Decides, for each mod a server has enabled, what clients must have. Called when the server's mods start and again when one is
/// switched on or off.
///
/// The default ([`ModShareDefault`]) is the mod's own word (`share` in `mod.toml`), and without one: required when the mod has
/// content (files that are not scripts), not required when it is scripts only. A game fills the slot with
/// [`ModsRegistry::share`](crate::ModsRegistry::share) to decide another way (from a prefs file, a game mode, a list).
pub trait ModShare {
    /// What clients must have of `info`, whose content is `content`.
    fn share(&self, info: &ModInfo, content: &ContentSummary) -> Share;
}

/// The default [`ModShare`]: the mod's `share`, else required when it has content.
#[derive(Clone, Copy, Debug, Default)]
pub struct ModShareDefault;

impl ModShare for ModShareDefault {
    fn share(&self, info: &ModInfo, content: &ContentSummary) -> Share {
        info.share.unwrap_or(if content.files > 0 {
            Share::Required
        } else {
            Share::None
        })
    }
}
