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
/// from. It names the screen, not the device: every reader below with a
/// 480×800 screen shares it, so a copy made for one is made for them all.
pub const X4: Profile = Profile {
    device: "X4",
    width: 480,
    height: 800,
    marker: "xteink-x4-images-v1:480x800:gray:jpeg85",
};
pub const X4_PRO: Profile = Profile {
    device: "X4 Pro",
    ..X4
};
pub const X4_CLASSIC: Profile = Profile {
    device: "X4 Classic",
    ..X4
};
pub const STICKY: Profile = Profile {
    device: "Sticky",
    ..X4
};
pub const PAPER_MONO: Profile = Profile {
    device: "PaperMono",
    ..X4
};
pub const METALIO: Profile = Profile {
    device: "Metalio E-INK4",
    ..X4
};
pub const PICCO: Profile = Profile {
    device: "BOOX Picco",
    ..X4
};

pub const X3: Profile = Profile {
    device: "X3",
    width: 528,
    height: 792,
    marker: "crosspoint-images-v1:528x792:gray:jpeg85",
};

pub const M5PAPER: Profile = Profile {
    device: "M5Paper",
    width: 540,
    height: 960,
    marker: "crosspoint-images-v1:540x960:gray:jpeg85",
};
pub const M5PAPER_S3: Profile = Profile {
    device: "M5Paper S3",
    ..M5PAPER
};
pub const LILYGO_T5: Profile = Profile {
    device: "LilyGo T5",
    ..M5PAPER
};

pub const EEGO_A4: Profile = Profile {
    device: "EEGO A4",
    width: 552,
    height: 768,
    marker: "crosspoint-images-v1:552x768:gray:jpeg85",
};

/// Every reader CrossPoint is offered for, with its screen in portrait as
/// crosspointreader.com and the firmware's board table (`BoardConfig.h` in
/// freeink-sdk) give it, and the other names it goes by: what `/api/status`
/// says (`X4` and `X3` on the shared build, the board's name on the others),
/// the site's firmware ids, and the names on the box. Matched without case,
/// spaces or punctuation.
pub const KNOWN: [(Profile, &[&str]); 12] = [
    (X4, &["xteink_x4", "Xteink X4"]),
    (X4_PRO, &["x4pro", "xteink_x4_pro", "Xteink X4 Pro"]),
    (X4_CLASSIC, &["x4c", "xteink_x4_classic", "Xteink X4C"]),
    (X3, &["xteink_x3", "xteink_x3_uc8279", "Xteink X3"]),
    (STICKY, &["Seeed Sticky", "reTerminal Sticky"]),
    (M5PAPER, &["m5paper_v11", "M5Paper v1.1"]),
    (M5PAPER_S3, &["m5papers3", "m5paper_s3"]),
    (PAPER_MONO, &["m5stack_paper_mono", "M5PaperMono"]),
    (LILYGO_T5, &["lilygo", "lilygo_t5s3"]),
    (METALIO, &["metalio", "metalio_eink4"]),
    (EEGO_A4, &["eego_a4", "EEGO Reader A4"]),
    (PICCO, &["picco"]),
];

/// The default when nothing says otherwise. Every device copy made before
/// there was a choice was made for this screen.
pub const DEFAULT: Profile = X4;

/// The profile for a device CrossPoint has named, if that screen is known.
pub fn for_device(name: &str) -> Option<Profile> {
    let squash = |name: &str| {
        name.chars()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_lowercase())
            .collect::<String>()
    };
    let name = squash(name);
    KNOWN
        .into_iter()
        .find(|(profile, names)| {
            std::iter::once(&profile.device)
                .chain(names.iter())
                .any(|known| squash(known) == name)
        })
        .map(|(profile, _)| profile)
}

/// The devices that can be named, for saying so when one cannot.
pub fn known() -> String {
    KNOWN
        .iter()
        .map(|(profile, _)| profile.device)
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
        // As CrossPoint 2.0 on an X4 Pro names itself, and as a person would.
        assert_eq!(for_device("xteink_x4_pro"), Some(X4_PRO));
        assert_eq!(for_device("X4 Pro"), Some(X4_PRO));
        assert_eq!(for_device("x4c"), Some(X4_CLASSIC));
        assert_eq!(for_device("m5paper_v11"), Some(M5PAPER));
        // Not a guess: a device CrossPoint is not offered for has no profile,
        // and the caller has to decide what to do about that.
        assert_eq!(for_device("m5stack_papercolor"), None);
        assert!(known().starts_with("X4, X4 Pro, X4 Classic, X3"));
        // The marker is frozen; copies already on devices are recognized by it.
        assert_eq!(X4.marker, "xteink-x4-images-v1:480x800:gray:jpeg85");
    }

    #[test]
    fn readers_with_one_screen_share_its_copies_and_no_others() {
        for (profile, _) in KNOWN {
            for (other, _) in KNOWN {
                let same = (profile.width, profile.height) == (other.width, other.height);
                assert_eq!(
                    profile.marker == other.marker,
                    same,
                    "{profile:?} {other:?}"
                );
            }
            assert!(profile
                .marker
                .contains(&format!(":{}x{}:", profile.width, profile.height)));
        }
    }
}
