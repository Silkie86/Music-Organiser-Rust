//! The key finder's weights. Made by `theorybench keyfit`; don't edit by hand, except the last two lines when
//! a test asks (another number the finder is built from changed).
//!
//! Fitted on: pop909cl, billboard at 0.25 (trust weights penalty 0.0001, corrections penalty 0.0001).
//! - POP909-CL: Yao, Chen, Dubnov and Berg-Kirkpatrick (2026), MIT licence
//! - The McGill Billboard charts: Burgoyne, Wild and Fujinaga (ISMIR 2011), CC0
//!
//! Only these numbers come from them.
//!
//! Right on pop909cl songs held out of the fit (cross-validation by song, every fold):
//! - Whole songs with one key: 90.7% right (778 cases)
//! - Whole songs that change key: 73.4% right (128 cases)
//! - 1-bar windows: 41.5% right (72383 cases)
//! - 2-bar windows: 56.8% right (37450 cases)
//! - 4-bar windows: 71.6% right (18582 cases)
//! - 8-bar windows: 81.4% right (9063 cases)
//! - 16-bar windows: 84.9% right (4307 cases)
//! - 1-bar openings: 78.6% right (485 cases)
//! - 2-bar openings: 77.9% right (870 cases)
//! - 4-bar openings: 80.0% right (901 cases)
//! - 8-bar openings: 87.2% right (905 cases)
//! - 16-bar openings: 90.3% right (905 cases)

use super::{Calibration, FEATURES, Thresholds};

/// Whether these weights were fitted (`false` for the placeholder).
pub(super) const FITTED: bool = true;

/// The sources learned from, by the names `theorybench` gives them.
pub(super) const LEARNED_FROM: &[&str] = &["pop909cl", "billboard"];

/// One weight per feature (see `Heard::features`): for each stream, twelve for the notes above a major
/// key's tonic, then twelve for a minor key's; last, one for minor keys as a whole.
#[rustfmt::skip]
pub(super) const WEIGHTS: [f64; FEATURES] = [
    // Tune, major: the tonic, then each semitone up.
    -7.145141891727707, -37.634150589500145, -11.93275857379799, -31.954446739672903, -7.993454064774948, -19.4896092011569, -35.71025164548764, -7.613545833412532, -30.585251125400294, -14.825029342566715, -30.84013474059544, -18.492111831640322,
    // Tune, minor: the tonic, then each semitone up.
    -6.26961768703578, -40.17786075058079, -14.998585790174287, -9.980312079774201, -32.53207556028695, -12.882565893889478, -39.09523383111634, -6.5417807849602525, -21.536311261677398, -32.182093472970884, -15.220177328422682, -28.855820130592342,
    // Harmony, major: the tonic, then each semitone up.
    -7.470995175140824, -32.4883747702625, -15.231347278158456, -27.105718379596908, -12.313907622568479, -11.893528765010634, -29.981011123962627, -9.425762533860608, -27.034460585216948, -13.3289963322712, -24.385905885840877, -17.5666000669091,
    // Harmony, minor: the tonic, then each semitone up.
    -6.790843997555377, -35.41657602723178, -18.033102284963967, -11.411027954648324, -32.70820029719625, -14.66126541116647, -38.96524184240161, -10.570014649720523, -13.68101076547806, -29.239697031867223, -12.50991818684101, -27.23661824839665,
    // Bass, major: the tonic, then each semitone up.
    0.8234097235825886, 3.1699184784201573, 1.5156995032216294, 2.903921887228087, 0.6285819065306981, 1.7336795272780592, 3.3961446698702966, 1.2064686066645192, 4.422800677467329, 1.1038114296561325, 1.692579016273768, 1.2808898983504777,
    // Bass, minor: the tonic, then each semitone up.
    0.8589754225250066, 2.4710413152414183, 1.5397193401520022, 0.6654314768506207, 4.707079998351199, 1.4538263411544015, 3.743824044798176, 1.7676089329920035, 2.7467537217669515, 2.408647655024956, 0.6512282973963306, 2.9349090327652703,
    // MajorChords, major: the tonic, then each semitone up.
    0.9720811576760313, -1.4657160309167536, -2.4537932256070367, 0.2773691592782692, -1.298308162773953, -0.7274180907593154, 0.34348188036756744, 1.0551125773748593, 0.5142156888666187, -1.6795745705789211, -2.956583608523443, -2.1759160453626754,
    // MajorChords, minor: the tonic, then each semitone up.
    -2.992808521880422, -3.153718329980341, -1.2080270556279964, -0.45933319725147703, -1.4193460380416472, -2.9059221090888068, -1.6608826183536614, 1.9408050036073943, -0.951320105123451, -1.3730311916594298, 0.9971964254236373, -1.6083464809478853,
    // MinorChords, major: the tonic, then each semitone up.
    -2.3472055618504344, -2.4666848399300516, -0.5474695696537392, -1.8551585214268034, 0.29955134349632595, -1.095231948068248, -1.4483075177704283, -3.041778428536835, -2.2820270037955757, 0.0774005857540283, -2.893113950105147, -3.567711676343256,
    // MinorChords, minor: the tonic, then each semitone up.
    1.1624047080125326, -2.5434559108976864, -2.9000709176385513, -3.1555980547420277, -0.060629885654152726, -0.9127908639313894, -2.8466803687283444, -0.21769811855375354, -4.27875447773414, 0.822771364395408, -2.495902973638498, -2.644021660136323,
    // FirstBassLoop, major: the tonic, then each semitone up.
    1.5589755983600517, 0.1766020182566212, -0.41665563167887015, -0.13791290105538667, -0.23343386539992528, 0.4450945931342496, -0.6353386582600103, 0.24765605267520086, 0.22258883145169656, -0.17826736986010036, 0.1819690102420532, -0.08972638355733666,
    // FirstBassLoop, minor: the tonic, then each semitone up.
    1.359258790223286, -0.20435948378435798, -0.6273724357247554, -0.032031095371879016, 0.1324109981531386, 0.20017085838825632, 0.45570316783997644, 0.09296316496809767, 0.28400857066906715, -0.29069689139602195, -0.1167607046404371, -0.5881228416633522,
    // FirstBassSong, major: the tonic, then each semitone up.
    0.9170636881767188, -1.1261806411274873, -1.0730640777475868, -0.8334089747985195, -0.9766315959053256, -0.1914263400070603, -0.9861111830255141, -0.3187993440281167, -0.8433685005163569, -1.0432212025293828, -0.45330118542048325, -0.6310861768076842,
    // FirstBassSong, minor: the tonic, then each semitone up.
    0.6273574257024617, -1.4179550318377856, -1.101028473649923, -0.5312135035801886, -0.6944721220437629, -0.43320707604492004, -0.9372792937351024, -0.7676350263678643, -0.2236833582435215, -0.8887600980648414, -0.7301640826105766, -1.4378299240840584,
    // LastBassSong, major: the tonic, then each semitone up.
    0.8081246579750411, 0.7375190925924757, 0.018541725006333387, 0.6766811282319795, -0.18823258534356924, 0.22710549140356193, -0.2222440339234193, 0.2683557874615255, 0.5105711320468757, -0.3865754164166303, 0.658683428611619, 0.2449976573434745,
    // LastBassSong, minor: the tonic, then each semitone up.
    0.5374552819367707, 0.24620985474470292, 0.17226048362940383, 0.02980566069056456, 1.2029679265513176, -0.05105293504838329, 0.5259587387721639, 0.7027862064274876, 0.4386388263942686, 0.08438873144046832, -0.3930431435090011, 0.5577881043396179,
    // Minor keys.
    -0.4883015159989866,
];
/// How much the scores are sharpened (see `Calibration`): the knots at 1 to 16 bars; melody only, bass only,
/// accompaniment; what a loop adds at 1 to 16 bars; what a song adds.
#[rustfmt::skip]
pub(super) const CALIBRATION: Calibration = Calibration([0.4196033634844185, 0.5001460667901763, 0.6902187807904953, 0.857598389050668, 0.8534363307365288, 0.6121091352940591, 0.3423938829179855, 0.37709603735453107, 0.39393117714090586, 0.1959827801061484, 0.02944192295992805, -0.058905415462031145, 0.01752919704006261, -0.12215648057097249]);
/// How likely each kind of answer must be (see `Thresholds`) at 1, 2, 4, 8, 16 and 32 bars, fitted on the chances
/// of songs held out (`theorybench` `answerfit`):
/// - 1 bar (366367 cases): one key from 0.87, raised from 0.74: 0.2% given, 12.1% wrong (up to 15.6%); key or its relative never; home note never.
/// - 2 bars (249465 cases): one key from 0.87, raised from 0.81: 0.3% given, 11.3% wrong (up to 16.8%); key or its relative never; home note from 0.88, raised from 0.87: 0.2% given, 10.7% wrong (up to 16.5%).
/// - 3 to 4 bars (139886 cases): one key from 0.87: 3.4% given, 9.7% wrong (up to 11.7%); key or its relative from 0.61: 42.7% given, 10.0% wrong (up to 11.1%); home note from 0.88: 0.6% given, 9.4% wrong (up to 12.6%).
/// - 5 to 8 bars (62332 cases): one key from 0.82, raised from 0.77: 20.0% given, 8.1% wrong (up to 9.6%); key or its relative from 0.01: 46.9% given, 7.4% wrong (up to 8.7%); home note from 0.86: 0.7% given, 9.5% wrong (up to 15.2%).
/// - 9 to 16 bars (26459 cases): one key from 0.82, raised from 0.76: 28.8% given, 6.8% wrong (up to 8.4%); key or its relative from 0.01: 49.4% given, 4.6% wrong (up to 6.0%); home note from 0.75: 4.5% given, 9.8% wrong (up to 12.9%).
/// - 17 bars or more, and songs (5815 cases): one key from 0.82: 47.9% given, 4.9% wrong (up to 6.2%); key or its relative from 0.01: 23.0% given, 4.3% wrong (up to 6.1%); home note from 0.72: 12.3% given, 10.0% wrong (up to 12.8%).
#[rustfmt::skip]
pub(super) const THRESHOLDS: Thresholds = Thresholds {
    one: [0.87, 0.87, 0.87, 0.82, 0.82, 0.82],
    pair: [1.0, 1.0, 0.61, 0.01, 0.01, 0.01],
    home: [1.0, 0.88, 0.88, 0.86, 0.75, 0.72],
};
/// Notes with less than this share of their time inside one scale have no key (see `Why::NotTonal`).
/// Notes in one scale a key needs: 0.750, as built in; 0.10% of these cases have less (the 0.5th percentile is 0.800).
pub(super) const NOT_TONAL_BELOW: f64 = 0.75;
/// The hash of every number the finder is built from (see `numbers_hash`) when this file was written, and the version
/// of the finder these numbers were first given with: a test checks both, so changing any number fails until the
/// version is bumped.
pub(super) const SHIPPED_HASH: u64 = 0x979a18887be71603;
pub(super) const HASH_VERSION: u32 = 4;
