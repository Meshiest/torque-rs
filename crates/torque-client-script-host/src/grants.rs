//! Which tiers a realm gets: those its server asked for, the player allowed, this build reviewed and has the host
//! module of.

use torque_client_script_format::{Grants, Tier};

/// The tiers whose host modules this build has (a cargo feature each), and the base.
pub const HOST_TIERS: Grants = Grants(
    1 | if cfg!(feature = "ui") {
        1 << Tier::Ui as u32
    } else {
        0
    } | if cfg!(feature = "net") {
        1 << Tier::Net as u32
    } else {
        0
    },
);

/// The tiers a security review passed: only these are ever granted to a server's request. A tier joins once its
/// review reports clean and the owner approves it.
pub const REVIEWED_TIERS: Grants = Grants(1);

/// The tiers one realm holds: never more than [`HOST_TIERS`], always the base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GrantSet(Grants);

impl GrantSet {
    /// What a server's request comes to: the tiers it names that the player allowed, this build reviewed and has the
    /// host module of. Names of no tier are left out ([`unknown_tiers`] lists them for the log).
    pub fn from_request<'a>(
        requested: impl IntoIterator<Item = &'a str>,
        allowed: Grants,
    ) -> GrantSet {
        let asked = requested
            .into_iter()
            .filter_map(Tier::from_name)
            .fold(Grants(1), Grants::with);
        GrantSet(Grants(
            (asked.0 & allowed.0 & REVIEWED_TIERS.0 & HOST_TIERS.0) | 1,
        ))
    }

    /// `grants` with the review left out: for tests and for working on a tier before its review, never for a server's
    /// request.
    #[cfg(feature = "unreviewed-tiers")]
    pub fn unreviewed(grants: Grants) -> GrantSet {
        GrantSet(Grants((grants.0 & HOST_TIERS.0) | 1))
    }

    /// The tiers.
    pub fn grants(self) -> Grants {
        self.0
    }
}

/// The names in `requested` that name no tier (a newer server's): each worth one log line.
pub fn unknown_tiers<'a>(requested: impl IntoIterator<Item = &'a str>) -> Vec<&'a str> {
    requested
        .into_iter()
        .filter(|name| Tier::from_name(name).is_none())
        .collect()
}
