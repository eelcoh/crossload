//! Which screen a device copy is made for.
//!
//! Optimizing means resizing a book's images to the screen that will show
//! them. CrossPoint runs on more than one reader, and a copy made for one
//! device's screen is the wrong size on another's — quietly, since it still
//! opens and still looks like a book.
//!
//! So a screen is named here only when its size is known from the hardware
//! itself. Guessing at a device's geometry would degrade every image to a size
//! no device has, which is worse than not optimizing at all.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Profile {
    /// What CrossPoint calls this device in `/api/status`.
    pub device: &'static str,
    pub width: u32,
    pub height: u32,
    /// Stamped into a device copy so it is recognized as one later and never
    /// optimized twice.
    pub marker: &'static str,
}

/// The XTeink X4. Its marker is frozen: every device copy already on a reader
/// or a card carries it, and renaming it would make those copies read as
/// originals, losing both the ◐ and their grouping with the books they came
/// from.
pub const X4: Profile = Profile {
    device: "X4",
    width: 480,
    height: 800,
    marker: "xteink-x4-images-v1:480x800:gray:jpeg85",
};

/// Every screen whose size is known. Another CrossPoint reader belongs here
/// once someone has measured it or CrossPoint has been asked what it reports.
pub const KNOWN: [Profile; 1] = [X4];

/// The default when nothing says otherwise. Every device copy made before
/// there was a choice was made for this screen.
pub const DEFAULT: Profile = X4;

/// The profile for a device CrossPoint has named, if that screen is known.
pub fn for_device(name: &str) -> Option<Profile> {
    let name = name.trim();
    KNOWN
        .into_iter()
        .find(|profile| profile.device.eq_ignore_ascii_case(name))
}

/// The devices that can be named, for saying so when one cannot.
pub fn known() -> String {
    KNOWN
        .iter()
        .map(|profile| profile.device)
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_screen_is_named_only_when_its_size_is_known() {
        assert_eq!(for_device("X4"), Some(X4));
        assert_eq!(for_device(" x4 "), Some(X4));
        // Not a guess: a device nobody has measured has no profile, and the
        // caller has to decide what to do about that.
        assert_eq!(for_device("M5Paper"), None);
        assert_eq!(known(), "X4");
        // The marker is frozen; copies already on devices are recognized by it.
        assert_eq!(X4.marker, "xteink-x4-images-v1:480x800:gray:jpeg85");
    }
}
