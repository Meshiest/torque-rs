//! A map as a map list shows it: the `MissionInfo` object's `name` and `desc` of its mission file, as a mission list reads them.

/// A map in a map list: its file and the name and description its mission's `MissionInfo` gives (as read: the list cleans them
/// for its rows). Empty text means the mission gives none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Listing {
    pub map: String,
    pub name: String,
    pub desc: String,
}
