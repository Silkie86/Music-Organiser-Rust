//! The key finder's weights. Made by `theorybench keyfit`; don't edit by hand, except the last two lines when
//! a test asks (another number the finder is built from changed).
//!
//! Fitted on: pop909cl, billboard at 0.25 (trust weights penalty 0.0001, corrections penalty 0.0001).
//! - POP909-CL: Yao, Chen, Dubnov and Berg-Kirkpatrick (2026), MIT licence
//! - The McGill Billboard charts: Burgoyne, Wild and Fujinaga (ISMIR 2011), CC0
//!
//! Only these numbers come from them.
//!
//! Then rescaled on packs: each stream's weights scaled by one number, and the minor weight and the calibration fitted
//! again (penalty 0.0001); nothing else comes from them. Solomon's own MIDI packs, on his computer: only the stream
//! scales, the minor weight and the calibration, by his leave (2026-11-01, rule 5).
//!
//! Pulled back to 0.55 of the way to the fit, the last point where every hand case keeps its gate: each weight and
//! calibration number is its built-in value moved that share of the way to the fitted one. Scales: Tune 0.673, Harmony
//! 0.656, Bass 0.612, MajorChords 0.725, MinorChords 0.748, FirstBassLoop 0.982, FirstBassSong 1.000, LastBassSong
//! 1.000; minor weight 0.621.
//!
//! Right on packs pieces held out of the fit (cross-validation by pack, every fold, each fold's fit pulled back the
//! same way):
//! - Whole loops: 55.0% right (39444 cases)
//! - Pieces cut from longer parts: 41.3% right (5339 cases)
//! - 1-bar windows: 38.9% right (119651 cases)
//! - 2-bar windows: 43.8% right (103316 cases)
//! - 4-bar windows: 46.7% right (64418 cases)
//! - 8-bar windows: 47.3% right (28713 cases)
//! - 16-bar windows: 47.4% right (4649 cases)
//! - 1-bar openings: 58.1% right (17958 cases)
//! - 2-bar openings: 54.5% right (27826 cases)
//! - 4-bar openings: 53.4% right (30387 cases)
//! - 8-bar openings: 51.2% right (21397 cases)
//! - 16-bar openings: 50.1% right (4081 cases)

use super::{Calibration, FEATURES, Thresholds};

/// Whether these weights were fitted (`false` for the placeholder).
pub(super) const FITTED: bool = true;

/// The sources learned from, by the names `theorybench` gives them.
pub(super) const LEARNED_FROM: &[&str] = &["pop909cl", "billboard", "packs"];

/// One weight per feature (see `Heard::features`): for each stream, twelve for the notes above a major
/// key's tonic, then twelve for a minor key's; last, one for minor keys as a whole.
#[rustfmt::skip]
pub(super) const WEIGHTS: [f64; FEATURES] = [
    // Tune, major: the tonic, then each semitone up.
    -4.811260280438523, -25.341371334967512, -8.035054899139908, -21.516879960070547, -5.382480660025383, -13.123543808046296, -24.04589271282544, -5.126665252653023, -20.594916957106648, -9.982597394574602, -20.766545657068892, -12.451867927214131,
    // Tune, minor: the tonic, then each semitone up.
    -4.221716378521938, -27.054206691970606, -10.09946354719154, -6.720353468612167, -21.90583271196403, -8.674618157990874, -26.3252078875965, -4.404980409200059, -14.50171326624482, -21.670168404510846, -10.24867465910663, -19.430385478352935,
    // Harmony, major: the tonic, then each semitone up.
    -4.901552688231533, -21.314895399922676, -9.992945979280247, -17.783455038499035, -8.078879131239379, -7.803077973423811, -19.669870241589134, -6.184031792739802, -17.736704394787914, -8.74485610983421, -15.99904694724307, -11.525053056860461,
    // Harmony, minor: the tonic, then each semitone up.
    -4.455320726525715, -23.236022693718603, -11.831114718977611, -7.486519993039264, -21.459118007115528, -9.618928028236134, -25.56422289439042, -6.934749990654054, -8.975804899243496, -19.18351066087027, -8.20747727455349, -17.869335512085666,
    // Bass, major: the tonic, then each semitone up.
    0.5037971639173083, 1.9394912320548578, 0.9273695575898067, 1.7767431803665958, 0.3845932015741339, 1.060739026962172, 2.077906058103439, 0.7381689151644893, 2.7060579612600373, 0.6753588788589187, 1.0355919825607258, 0.783703033371545,
    // Bass, minor: the tonic, then each semitone up.
    0.5255577743968247, 1.5118883963680294, 0.9420659175873871, 0.4071393392830437, 2.8799921662127037, 0.8895129199046932, 2.290631118331173, 1.0814984835030934, 1.6805809979807762, 1.4737132957307568, 0.39844922873085487, 1.7957024367275758,
    // MajorChords, major: the tonic, then each semitone up.
    0.7049090729955467, -1.0628706466220568, -1.7793793186163906, 0.20113550746301012, -0.9414740695843697, -0.527490537172031, 0.24907744787432917, 0.7651196846581908, 0.3728858456893761, -1.217951139392683, -2.1439800517280174, -1.5778754174390877,
    // MajorChords, minor: the tonic, then each semitone up.
    -2.170248712417686, -2.28693319165888, -0.8760063140942695, -0.3330875572618899, -1.0292452353749693, -2.1072426349793885, -1.2043965852162755, 1.4073835761087299, -0.689854101305489, -0.9956598138580339, 0.7231215236394055, -1.1662997662160923,
    // MinorChords, major: the tonic, then each semitone up.
    -1.7547496102293625, -1.8440712359311227, -0.4092832897025788, -1.38689970120086, 0.22394187019115197, -0.8187854805815964, -1.0827415773062383, -2.274005991904346, -1.7060227108043675, 0.05786397659024463, -2.162865774863748, -2.667188922532467,
    // MinorChords, minor: the tonic, then each semitone up.
    0.8690032272698494, -1.9014645929710343, -2.1680667407552887, -2.3590965131463624, -0.0453263531533224, -0.6823941791624775, -2.1281524501567874, -0.1627491408884989, -3.198758078159985, 0.615096416967642, -1.8659144479487808, -1.9766466358849926,
    // FirstBassLoop, major: the tonic, then each semitone up.
    1.530275907401123, 0.17335089402349027, -0.4089852820736167, -0.13537401741685984, -0.22913650513118633, 0.4369007013992663, -0.6236425013955259, 0.2430968715158785, 0.21849112095494932, -0.17498558762538938, 0.1786190833005778, -0.0880745812573733,
    // FirstBassLoop, minor: the tonic, then each semitone up.
    1.3342357512137901, -0.20059736330260894, -0.615822931652728, -0.0314414244756726, 0.12997340084208645, 0.19648584767944877, 0.4473139793884253, 0.09125177570199569, 0.27878016413309337, -0.28534535737934885, -0.1146112186941193, -0.577295896187502,
    // FirstBassSong, major: the tonic, then each semitone up.
    0.9170636881767188, -1.1261806411274873, -1.0730640777475868, -0.8334089747985195, -0.9766315959053256, -0.1914263400070603, -0.9861111830255141, -0.3187993440281167, -0.8433685005163569, -1.0432212025293828, -0.45330118542048325, -0.6310861768076842,
    // FirstBassSong, minor: the tonic, then each semitone up.
    0.6273574257024617, -1.4179550318377856, -1.101028473649923, -0.5312135035801886, -0.6944721220437629, -0.43320707604492004, -0.9372792937351024, -0.7676350263678643, -0.2236833582435215, -0.8887600980648414, -0.7301640826105766, -1.4378299240840584,
    // LastBassSong, major: the tonic, then each semitone up.
    0.8081246579750411, 0.7375190925924757, 0.018541725006333387, 0.6766811282319795, -0.18823258534356924, 0.22710549140356193, -0.2222440339234193, 0.2683557874615255, 0.5105711320468757, -0.3865754164166303, 0.658683428611619, 0.2449976573434745,
    // LastBassSong, minor: the tonic, then each semitone up.
    0.5374552819367707, 0.24620985474470292, 0.17226048362940383, 0.02980566069056456, 1.2029679265513176, -0.05105293504838329, 0.5259587387721639, 0.7027862064274876, 0.4386388263942686, 0.08438873144046832, -0.3930431435090011, 0.5577881043396179,
    // Minor keys.
    0.6214692775061905,
];
/// How much the scores are sharpened (see `Calibration`): the knots at 1 to 16 bars; melody only, bass only,
/// accompaniment; what a loop adds at 1 to 16 bars; what a song adds.
#[rustfmt::skip]
pub(super) const CALIBRATION: Calibration = Calibration([0.4196033634844185, 0.5001460667901763, 0.6902187807904953, 0.857598389050668, 0.8534363307365288, 0.48491518048234056, 0.37162694021560905, 0.3301708909342936, 0.4220129984571627, 0.1805388006859945, -0.05873970406474617, -0.16342764893838185, -0.17335501974014542, -0.12215648057097249]);
/// How likely each kind of answer must be (see `Thresholds`) at 1, 2, 4, 8, 16 and 32 bars: kept as they were
/// fitted before the rescale, and how they come out on the packs pieces held out:
/// - 1 bar (157228 cases): one key from 0.87: 0.5% given, 18.2% wrong (up to 21.4%); key or its relative never; home note never.
/// - 2 bars (141322 cases): one key from 0.87: 0.3% given, 20.7% wrong (up to 25.5%); key or its relative never; home note from 0.88: 0.1% given, 11.7% wrong (up to 17.9%).
/// - 3 to 4 bars (108276 cases): one key from 0.87: 0.9% given, 16.6% wrong (up to 20.4%); key or its relative from 0.61: 29.5% given, 34.1% wrong (up to 35.1%); home note from 0.88: 0.6% given, 24.6% wrong (up to 29.5%).
/// - 5 to 8 bars (46605 cases): one key from 0.82: 6.7% given, 18.5% wrong (up to 20.6%); key or its relative from 0.50: 34.2% given, 33.7% wrong (up to 35.0%); home note from 0.86: 2.3% given, 40.5% wrong (up to 44.7%).
/// - 9 to 16 bars (6077 cases): one key from 0.82: 5.8% given, 19.0% wrong (up to 24.6%); key or its relative from 0.50: 35.1% given, 37.7% wrong (up to 40.7%); home note from 0.75: 7.0% given, 25.9% wrong (up to 32.5%).
/// - 17 bars or more, and songs (2 cases): one key from 0.82: none given; key or its relative from 0.50: none given; home note from 0.72: 50.0% given, 0.0% wrong (up to 79.3%).
#[rustfmt::skip]
pub(super) const THRESHOLDS: Thresholds = Thresholds {
    one: [0.87, 0.87, 0.87, 0.82, 0.82, 0.82],
    pair: [1.0, 1.0, 0.61, 0.5, 0.5, 0.5],
    home: [1.0, 0.88, 0.88, 0.86, 0.75, 0.72],
};
/// Notes with less than this share of their time inside one scale have no key (see `Why::NotTonal`).
/// Notes in one scale a key needs: 0.750, as built in; 1.64% of these cases have less (the 0.5th percentile is 0.667).
pub(super) const NOT_TONAL_BELOW: f64 = 0.75;
/// The hash of every number the finder is built from (see `numbers_hash`) when this file was written, and the version
/// of the finder these numbers were first given with: a test checks both, so changing any number fails until the
/// version is bumped.
pub(super) const SHIPPED_HASH: u64 = 0x102508ef13654ccd;
pub(super) const HASH_VERSION: u32 = 5;
