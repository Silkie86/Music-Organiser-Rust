//! What the Space page's clean-ups share while the app runs: which of what a look found you ticked, where Remove has
//! got to, what went and what stayed, and what Put back brought back (the page's question 5: "every clean-up result has
//! a Put back button"). Spare plugin copies (`crate::space_spare`), old copies (`crate::space_old`) and your songs' old
//! autosaves and caches (`crate::space_songs`) each say their own words around it.

use std::collections::{HashMap, HashSet};
use std::fmt::Debug;
use std::path::PathBuf;

use mo_space::clean::{Done, NEEDS_ADMIN, Problem, Removed};
use mo_space::size::size_text;

use crate::songs::thousands;
use crate::space_bin::{BIN, PutBacks, yourself_words};

/// What a card says, above Remove, about the ones in Program Files (Windows guards them).
pub const ADMIN: &str = "The ones in Program Files go together at the end, so Windows asks once for administrator \
    rights.";

/// What a card says, once they went, of putting back the ones from Program Files: Put back asks Windows once for all
/// of them, as Remove did.
pub const ADMIN_BACK: &str = "For the ones from Program Files, Windows asks once for administrator rights.";

/// How far Remove has got, for its card while it runs: of what was sent so far, what went to the Recycle Bin, what
/// waits for Windows' administrator step (it asks once, at the end) and what stays. [`mo_space::clean::Heard`] tells it
/// of each send ([`Tally::heard`]). What a clean-up turns away before sending it (changed since the look, say) isn't
/// heard: that takes no time, and Remove's result says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tally {
    /// Whether Windows can be asked here, so that what it guards waits for the step rather than staying.
    can_ask: bool,
    pub went: usize,
    pub waits: usize,
    pub stays: usize,
}

impl Tally {
    /// A tally for a bin whose administrator step `can_ask` ([`mo_space::admin::Admin::can_ask`]).
    pub fn new(can_ask: bool) -> Self {
        Tally { can_ask, went: 0, waits: 0, stays: 0 }
    }

    /// Notes how one send went.
    pub fn heard(&mut self, sent: &Result<(), String>) {
        match sent {
            Ok(()) => self.went += 1,
            Err(why) if why == NEEDS_ADMIN && self.can_ask => self.waits += 1,
            Err(_) => self.stays += 1,
        }
    }

    /// How far it has got of `of`, for the card's bar.
    pub fn fraction(&self, of: usize) -> f32 {
        (self.went + self.waits + self.stays) as f32 / of.max(1) as f32
    }

    /// What the card says: its `note` (what it said as Remove started), then how far it has got of `of`.
    pub fn words(&self, note: &str, of: usize) -> String {
        let verb = |n: usize, one: &'static str, many: &'static str| if n == 1 { one } else { many };
        let mut words = format!(
            "{note} {} of {} {} gone so far",
            thousands(self.went),
            thousands(of),
            verb(self.went, "has", "have")
        );
        if self.stays > 0 {
            words += &format!(" and {} {}", thousands(self.stays), verb(self.stays, "stays", "stay"));
        }
        words.push('.');
        if self.waits > 0 {
            let wait = verb(self.waits, "waits", "wait");
            words += &format!(" {} {wait} for Windows to ask, once, at the end.", thousands(self.waits));
        }
        words
    }
}

/// Something a clean-up lists, that Remove can send to the Recycle Bin.
pub trait Removable: Clone + Debug + PartialEq + Eq {
    /// Where it is: Remove's result says how each one went by this.
    fn path(&self) -> &str;
    /// The room it takes.
    fn bytes(&self) -> u64;
    /// Whether it can go: only one that can has a tick.
    fn can_go(&self) -> bool;
    /// Whether it's ticked out of the box: every one that can go, unless the clean-up wants you to look at it first.
    fn ticked_at_first(&self) -> bool {
        self.can_go()
    }
}

/// What a look found, as Remove picks from it.
pub trait Looked {
    type Item: Removable;
    /// Each one found, in the order the card lists them.
    fn items(&self) -> &[Self::Item];
}

/// The room the ones that can go take, of `items`.
pub fn free_bytes<'a, T: Removable + 'a>(items: impl IntoIterator<Item = &'a T>) -> u64 {
    items.into_iter().filter(|c| c.can_go()).fold(0u64, |sum, c| sum.saturating_add(c.bytes()))
}

/// A removal that ran: what was picked, when it ran (as the list of what the app put in the Recycle Bin keeps it), and
/// what went and what stayed, with why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sent<T> {
    pub picked: Vec<T>,
    pub when: i64,
    /// What it did. Kept as it was made: what's worked out of it below is only made once.
    done: Done,
    /// Where each one that went was, with the room it freed, and where each one `done` says something of is, with the
    /// first thing it says (by where that is in `done.problems`): so a card listing thousands asks of each one quickly.
    went: HashMap<String, u64>,
    why: HashMap<String, usize>,
}

impl<T: Removable> Sent<T> {
    /// A removal of `picked` that ran at `when`, and what it did. One that went inside another that went (`went_with`)
    /// went too, its room counted in that one's.
    pub fn new(picked: Vec<T>, when: i64, done: Done) -> Self {
        let mut went: HashMap<String, u64> = done.removed.iter().map(|r| (r.path.clone(), r.bytes)).collect();
        for (inner, outer) in &done.went_with {
            if went.contains_key(outer) {
                went.entry(inner.clone()).or_insert(0);
            }
        }
        let mut why = HashMap::new();
        for (n, p) in done.problems.iter().enumerate() {
            why.entry(p.path.clone()).or_insert(n);
        }
        // What's said of the one it went inside is said of it too (it wasn't found in the bin, say).
        for (inner, outer) in &done.went_with {
            if let Some(&n) = why.get(outer) {
                why.entry(inner.clone()).or_insert(n);
            }
        }
        Sent { picked, when, done, went, why }
    }

    /// What it did.
    pub fn done(&self) -> &Done {
        &self.done
    }

    /// Whether `item` went: to the bin, or for good.
    pub fn went(&self, item: &T) -> bool {
        self.went.contains_key(item.path())
    }

    /// The room `item` freed: as it was when it went (none when it stayed, or went inside another).
    pub fn freed(&self, item: &T) -> u64 {
        self.went.get(item.path()).copied().unwrap_or(0)
    }

    /// Why `item` stayed, or why it isn't in the bin though it went.
    pub fn why(&self, item: &T) -> Option<&str> {
        let n = *self.why.get(item.path())?;
        self.done.problems.get(n).map(|p| p.why.as_str())
    }

    /// Whether `item` is in the bin: it went, and was found there.
    pub fn in_bin(&self, item: &T) -> bool {
        self.went(item) && self.why(item).is_none()
    }

    /// What went that was found in the bin: what Put back puts back.
    pub fn binned(&self) -> impl Iterator<Item = &Removed> {
        self.done.removed.iter().filter(|r| !self.why.contains_key(&r.path))
    }

    /// How many went but weren't found in the bin: Windows removed them for good (too big for it), they went to
    /// another account's bin, or the bin couldn't be read to check.
    pub fn lost(&self) -> usize {
        self.done.removed.len() - self.binned().count()
    }

    /// How many picked stayed.
    pub fn stayed(&self) -> usize {
        self.picked.iter().filter(|c| !self.went(c)).count()
    }

    /// What Put back puts back of this removal: what went that was found in the bin. Put back says which of those
    /// aren't there any more (put back already, or emptied from the bin since).
    pub fn coming_back(&self) -> Done {
        let removed: Vec<Removed> = self.binned().cloned().collect();
        let back: HashSet<&str> = removed.iter().map(|r| r.path.as_str()).collect();
        let went_with = self.done.went_with.iter().filter(|(_, with)| back.contains(with.as_str())).cloned().collect();
        Done { removed, problems: Vec::new(), went_with }
    }
}

/// Where removing has got to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Stage<T> {
    /// Picking which to remove: out of the box, and again after Measure again.
    #[default]
    Picking,
    /// The ticked ones are being checked again and sent to the Recycle Bin.
    Sending(Vec<T>),
    /// They were sent: the card shows what went and what stayed.
    Sent(Sent<T>),
    /// What went is being put back.
    PuttingBack(Sent<T>),
    /// What went was put back: what couldn't come back, and why.
    PutBack(Sent<T>, Vec<Problem>),
}

/// How removing goes while the app runs: the ticks you changed, and where Remove has got to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Removing<T> {
    /// By path: what you ticked or unticked. The rest are as they are out of the box
    /// ([`Removable::ticked_at_first`]). A tick you changed stays through a new look, until Remove: what you ticked
    /// goes with it ([`start`](Self::start)), and what you unticked stays unticked.
    ticks: HashMap<String, bool>,
    stage: Stage<T>,
}

impl<T> Default for Removing<T> {
    fn default() -> Self {
        Removing { ticks: HashMap::new(), stage: Stage::Picking }
    }
}

impl<T: Removable> Removing<T> {
    pub fn stage(&self) -> &Stage<T> {
        &self.stage
    }

    /// Whether `item` is ticked: one that can go, as you left it, or as it is out of the box.
    pub fn ticked(&self, item: &T) -> bool {
        item.can_go() && self.ticks.get(item.path()).copied().unwrap_or_else(|| item.ticked_at_first())
    }

    /// Ticks or unticks the ones at `paths` (one's own tick, or a group's for all of its), while picking.
    pub fn tick(&mut self, paths: &[PathBuf], ticked: bool) {
        if self.stage != Stage::Picking {
            return;
        }
        for path in paths {
            self.ticks.insert(path.to_string_lossy().into_owned(), ticked);
        }
    }

    /// The ticked ones, in the order they were found: what Remove sends.
    pub fn ticked_copies<F: Looked<Item = T>>(&self, found: Option<&F>) -> Vec<T> {
        self.each_ticked(found).cloned().collect()
    }

    /// Each ticked one, in the order it was found.
    pub fn each_ticked<'a, F: Looked<Item = T>>(&'a self, found: Option<&'a F>) -> impl Iterator<Item = &'a T> {
        found.into_iter().flat_map(|f| f.items()).filter(|c| self.ticked(c))
    }

    /// Whether Remove or Put back is on: neither can start again, and Measure again waits.
    pub fn busy(&self) -> bool {
        matches!(self.stage, Stage::Sending(_) | Stage::PuttingBack(_))
    }

    /// Remove starts on `picked` ([`ticked_copies`](Self::ticked_copies)). The ticks you gave go with it: one that
    /// isn't ticked out of the box (with nothing to check it against) goes another time only once you tick it again,
    /// having looked in it again, even after Put back. The ones you unticked stay unticked.
    pub fn start(&mut self, picked: Vec<T>) {
        if self.stage == Stage::Picking {
            self.ticks.retain(|_, ticked| !*ticked);
            self.stage = Stage::Sending(picked);
        }
    }

    /// Remove has finished: it ran at `when`, and `done` says what went and what stayed.
    pub fn sent(&mut self, when: i64, done: Done) {
        if let Stage::Sending(picked) = &mut self.stage {
            let picked = std::mem::take(picked);
            self.stage = Stage::Sent(Sent::new(picked, when, done));
        }
    }

    /// What the card's Put back puts back: the removal on show, once something it sent is in the bin.
    pub fn to_put_back(&self) -> Option<&Sent<T>> {
        match &self.stage {
            Stage::Sent(sent) if sent.binned().next().is_some() => Some(sent),
            _ => None,
        }
    }

    /// Put back starts on the removal that ran at `when` (from this card, or the Recycle Bin card's): when it's the one
    /// on show, its card says so.
    pub fn putting_back(&mut self, when: i64) {
        if let Stage::Sent(sent) = &self.stage
            && sent.when == when
        {
            self.stage = Stage::PuttingBack(sent.clone());
        }
    }

    /// Put back of the removal that ran at `when` has finished: `problems` says what couldn't come back.
    pub fn put_back(&mut self, when: i64, problems: Vec<Problem>) {
        if let Stage::Sent(sent) | Stage::PuttingBack(sent) = &self.stage
            && sent.when == when
        {
            self.stage = Stage::PutBack(sent.clone(), problems);
        }
    }

    /// Measure again: the card goes back to picking, for what the new look finds. Not while Remove or Put back is on.
    pub fn again(&mut self) {
        if !self.busy() {
            self.stage = Stage::Picking;
        }
    }

    /// Whether the card shows how a removal went (then it has Open Recycle Bin too).
    pub fn shows_result(&self) -> bool {
        matches!(self.stage, Stage::Sent(_) | Stage::PuttingBack(_) | Stage::PutBack(..))
    }

    /// Remove's button: its words and whether it can be pressed now (not while a look is on or anything else goes to
    /// the bin, `wait`, nor while something is put back: one thing goes or comes back at a time). None when none can
    /// go, and once a removal's result is on show.
    pub fn remove_button<F: Looked<Item = T>>(
        &self,
        found: Option<&F>,
        wait: bool,
        put_backs: &PutBacks,
    ) -> Option<(String, bool)> {
        match &self.stage {
            Stage::Sending(_) => Some(("Removing…".to_owned(), false)),
            Stage::Picking if found.is_some_and(|f| f.items().iter().any(Removable::can_go)) => {
                let mut ticked = self.each_ticked(found).peekable();
                if ticked.peek().is_none() {
                    Some(("Nothing ticked".to_owned(), false))
                } else {
                    let can = !wait && put_backs.going.is_none();
                    Some((format!("Remove {}", size_text(free_bytes(ticked))), can))
                }
            }
            _ => None,
        }
    }

    /// Put back's button, once something went to the bin: None where the app can't put things back (`can_put_back`: a
    /// Mac). It waits while another clean-up is put back from the Recycle Bin card.
    pub fn put_back_button(&self, can_put_back: bool, put_backs: &PutBacks) -> Option<(String, bool)> {
        match &self.stage {
            Stage::Sent(sent) if can_put_back && sent.binned().next().is_some() => {
                Some(("Put back".to_owned(), put_backs.going.is_none()))
            }
            Stage::PuttingBack(_) => Some(("Putting back…".to_owned(), false)),
            _ => None,
        }
    }
}

/// Every one picked stays, for `why`: what Remove says when it couldn't check them again (what the look read couldn't
/// be read again, say), or couldn't start.
pub fn stayed<T: Removable>(picked: &[T], why: &str) -> Done {
    let problems = picked.iter().map(|t| Problem { path: t.path().to_owned(), why: why.to_owned() }).collect();
    Done { problems, ..Done::default() }
}

/// What Remove did when it stopped with an error part way (logged as a crash): what had gone to the Recycle Bin by
/// then, as each one was noted going (`went`, [`crate::binned::Going::went_so_far`]), but for what's still where it
/// really was (`still_there`: what waited for Windows' administrator step and didn't go); every other one picked
/// stays, saying so ([`crashed_words`]), or, when it isn't where it was any more (it may have gone just as Remove
/// stopped), saying that ([`crashed_gone_words`]). Looks at each place on the disk: call it off the window's thread.
pub fn after_crash<T: Removable>(picked: &[T], went: Vec<Removed>, still_there: impl Fn(&str) -> bool) -> Done {
    let removed: Vec<Removed> = went.into_iter().filter(|r| !still_there(&r.real)).collect();
    let gone: HashSet<&str> = removed.iter().map(|r| r.path.as_str()).collect();
    let (stays, moved) = (crashed_words(), crashed_gone_words());
    let problems = picked
        .iter()
        .filter(|t| !gone.contains(t.path()))
        .map(|t| {
            let why = if still_there(t.path()) { &stays } else { &moved };
            Problem { path: t.path().to_owned(), why: why.clone() }
        })
        .collect();
    Done { removed, problems, went_with: Vec::new() }
}

/// Why one picked stays when Remove stopped with an error (logged as a crash) before it was sent.
pub fn crashed_words() -> String {
    "Remove stopped with an error (the log says which) before this went, so it stays.".to_owned()
}

/// Why one picked isn't said to have gone, though it isn't where it was, when Remove stopped with an error (logged as a
/// crash): it may have gone just as it stopped, without being noted.
pub fn crashed_gone_words() -> String {
    format!(
        "Remove stopped with an error (the log says which) as this went: it isn't where it was, so it may be in the \
         {BIN}."
    )
}

/// How many places the log names of what one removal sent, and of what stayed for each reason (and how many reasons it
/// gives): the rest are counted, so a clean-up of thousands doesn't fill the log.
pub const LOGGED: usize = 50;

/// What the log says of a removal (`done`) of things it calls `one` ("an old copy", and "An old copy" to start a line)
/// and `many` ("old copies"), each line with whether it's a warning: what went, then what stayed or went but isn't in
/// the bin, by why (in the order the reasons first come, as one line each when only one is said to be so).
pub fn log_lines(done: &Done, one: (&str, &str), many: &str) -> Vec<(bool, String)> {
    let mut lines: Vec<(bool, String)> =
        done.removed.iter().take(LOGGED).map(|r| (false, format!("Sent {} to the {BIN}: {}", one.0, r.path))).collect();
    if done.removed.len() > LOGGED {
        lines.push((false, format!("…and sent {} more {many} to the {BIN}", thousands(done.removed.len() - LOGGED))));
    }
    let went: HashSet<&str> = done.removed.iter().map(|r| r.path.as_str()).collect();
    let mut groups: Vec<((&str, &str), Vec<&str>)> = Vec::new();
    let mut at: HashMap<(&str, &str), usize> = HashMap::new();
    for p in &done.problems {
        let how = if went.contains(p.path.as_str()) { "went" } else { "stayed" };
        let n = *at.entry((how, p.why.as_str())).or_insert_with(|| {
            groups.push(((how, p.why.as_str()), Vec::new()));
            groups.len() - 1
        });
        if let Some((_, paths)) = groups.get_mut(n) {
            paths.push(p.path.as_str());
        }
    }
    for ((how, why), paths) in groups.iter().take(LOGGED) {
        if let [path] = paths.as_slice() {
            lines.push((true, format!("{} {how}: {path}: {why}", one.1)));
            continue;
        }
        lines.push((true, format!("{} {many} {how}: {why}", thousands(paths.len()))));
        lines.extend(paths.iter().take(LOGGED).map(|path| (true, format!("  {path}"))));
        if paths.len() > LOGGED {
            lines.push((true, format!("  …and {} more", thousands(paths.len() - LOGGED))));
        }
    }
    if groups.len() > LOGGED {
        let rest: usize = groups.iter().skip(LOGGED).map(|(_, paths)| paths.len()).sum();
        let reasons = thousands(groups.len() - LOGGED);
        lines.push((
            true,
            format!("…and {} more {many} stayed or aren't in the {BIN}, for {reasons} other reasons", thousands(rest)),
        ));
    }
    lines
}

/// Parts of a sentence, each saying something of its own, as one sentence: "A", "A, and B", "A, B, and C".
pub fn clauses(said: &[String]) -> String {
    match said {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{}, and {last}", rest.join(", ")),
    }
}

/// How a clean-up counts what it sent, as its lines say it: "10 spare copies". A closure counting `n` of them is one.
pub trait Noun {
    /// `n` of them: "10 spare copies".
    fn count(&self, n: usize) -> String;
    /// The `n` that are in the bin. A clean-up whose things are of several kinds counts them by kind ("61 old autosaves
    /// and 1 cache").
    fn in_bin(&self, n: usize) -> String {
        self.count(n)
    }
}

impl<F: Fn(usize) -> String> Noun for F {
    fn count(&self, n: usize) -> String {
        self(n)
    }
}

/// "Freed 9.1 GB" once something went; "" when nothing did.
pub fn freed_words<T>(sent: &Sent<T>) -> String {
    if sent.done.removed.is_empty() { String::new() } else { format!("Freed {}", size_text(sent.done.bytes())) }
}

/// How a removal went, as the card's big figures' note and the line say it, counting with `noun` ("1 spare copy", "10
/// spare copies"): "10 spare copies are in the Recycle Bin", "8 spare copies are in the Recycle Bin, 1 went but wasn't
/// found there, and 2 stayed".
pub fn sent_words<T: Removable>(sent: &Sent<T>, noun: &dyn Noun) -> String {
    let (in_bin, lost, stayed) = (sent.binned().count(), sent.lost(), sent.stayed());
    if in_bin + lost == 0 {
        return if stayed == 0 { String::new() } else { "None could go: each one's line says why".to_owned() };
    }
    let mut said = Vec::new();
    if in_bin > 0 {
        said.push(format!("{} {} in the {BIN}", noun.in_bin(in_bin), if in_bin == 1 { "is" } else { "are" }));
    }
    if lost > 0 {
        let (who, there) = if in_bin > 0 {
            (thousands(lost), "there".to_owned())
        } else {
            (noun.count(lost), format!("in the {BIN}"))
        };
        said.push(format!("{who} went but {} found {there}", if lost == 1 { "wasn't" } else { "weren't" }));
    }
    if stayed > 0 {
        said.push(format!("{} stayed", thousands(stayed)));
    }
    clauses(&said)
}

/// How a put back went: "All 10 are back where they were", "8 are back where they were, and 2 couldn't come back".
/// Of what went, only what was found in the bin is put back.
pub fn back_words<T: Removable>(sent: &Sent<T>, problems: &[Problem]) -> String {
    let went = sent.binned().count();
    let stuck = problems.len().min(went);
    match (went - stuck, stuck) {
        (0, 0) => String::new(),
        (1, 0) => "It's back where it was".to_owned(),
        (n, 0) => format!("All {} are back where they were", thousands(n)),
        (0, _) => "None could come back: each one's line says why".to_owned(),
        (1, m) => format!("1 is back where it was, and {} couldn't come back", thousands(m)),
        (n, m) => format!("{} are back where they were, and {} couldn't come back", thousands(n), thousands(m)),
    }
}

/// What a picked one's line says once Remove has run: why it stayed, that it's in the bin (or why it isn't, though it
/// went), and once Put back has run, whether it's back.
pub fn result_words<T: Removable>(item: &T, sent: &Sent<T>, stage: &Stage<T>) -> (String, bool) {
    if let Some(why) = sent.why(item) {
        return (why.to_owned(), true);
    }
    if !sent.went(item) {
        return (String::new(), false);
    }
    match stage {
        Stage::PuttingBack(_) => ("Putting back…".to_owned(), false),
        Stage::PutBack(_, problems) => match problems.iter().find(|p| p.path == item.path()) {
            Some(p) => (format!("It couldn't come back: {}", p.why), true),
            None => ("Back where it was.".to_owned(), false),
        },
        _ => (format!("In the {BIN}."), false),
    }
}

/// Whether a picked one is gone from where it was: it went, and isn't back.
pub fn gone<T: Removable>(item: &T, sent: &Sent<T>, stage: &Stage<T>) -> bool {
    let back = sent.in_bin(item)
        && matches!(stage, Stage::PutBack(_, problems) if !problems.iter().any(|p| p.path == item.path()));
    sent.went(item) && !back
}

/// The activity card's line once Remove is done, counting with `noun` ("1 copy", "10 copies"): what was freed, what's
/// in the bin, what went but wasn't found there, and what stayed. Err when nothing went (the card then fails).
pub fn removed_words(done: &Done, noun: &dyn Noun) -> Result<String, String> {
    let said: HashSet<&str> = done.problems.iter().map(|p| p.path.as_str()).collect();
    let went: HashSet<&str> = done.removed.iter().map(|r| r.path.as_str()).collect();
    let in_bin = done.removed.iter().filter(|r| !said.contains(r.path.as_str())).count();
    let lost = done.removed.len() - in_bin;
    let stayed = done.problems.iter().filter(|p| !went.contains(p.path.as_str())).count();
    match (done.removed.len(), stayed) {
        (0, 0) => return Err("Nothing was removed.".to_owned()),
        (0, 1) => return Err("It couldn't go: the Space page says why.".to_owned()),
        (0, _) => return Err("None could go: the Space page says why.".to_owned()),
        _ => {}
    }
    let mut said = Vec::new();
    if in_bin > 0 {
        said.push(format!("{} {} in the {BIN}", noun.in_bin(in_bin), if in_bin == 1 { "is" } else { "are" }));
    }
    if lost > 0 {
        let (who, there) = if in_bin > 0 {
            (thousands(lost), "there".to_owned())
        } else {
            (noun.count(lost), format!("in the {BIN}"))
        };
        said.push(format!("{who} went but {} found {there}", if lost == 1 { "wasn't" } else { "weren't" }));
    }
    let stayed = match stayed {
        0 => String::new(),
        m => format!("; {} stayed (the Space page says why)", thousands(m)),
    };
    Ok(format!("Freed {}: {}{stayed}.", size_text(done.bytes()), clauses(&said)))
}

/// A card's big figure, its note and the note under its list once Remove has run, counting with `noun`: what was freed
/// and how it went, and what Put back does (`can_put_back`: the app can put things back here, which a Mac's Trash
/// doesn't let it; for what went as administrator, Windows asks once); then, once Put back has run, what came back.
pub fn result_figures<T: Removable>(
    sent: &Sent<T>,
    stage: &Stage<T>,
    can_put_back: bool,
    noun: &dyn Noun,
) -> (String, String, String) {
    if let Stage::PutBack(_, problems) = stage {
        let note = if problems.is_empty() { String::new() } else { yourself_words() };
        return ("Put back".to_owned(), back_words(sent, problems), note);
    }
    // What went as administrator comes back the same way (it has the stamp the step checked).
    let from_guarded = sent.binned().any(|r| r.stamp.is_some());
    let note = match (sent.binned().next().is_some(), can_put_back) {
        (false, _) => String::new(),
        (true, true) if from_guarded => format!(
            "They're in the {BIN}. Put back returns each one to where it was, until you empty the {BIN}. {ADMIN_BACK} \
             The app never empties it for you."
        ),
        (true, true) => format!(
            "They're in the {BIN}. Put back returns each one to where it was, until you empty the {BIN}. The app never \
             empties it for you."
        ),
        (true, false) => format!(
            "They're in the {BIN}. Until you empty it, Finder's Put Back returns each one to where it was: select them \
             there and choose Put Back. The app never empties it for you."
        ),
    };
    let big = if sent.done.removed.is_empty() { "Nothing freed".to_owned() } else { freed_words(sent) };
    let big_note =
        if matches!(stage, Stage::PuttingBack(_)) { "Putting them back…".to_owned() } else { sent_words(sent, noun) };
    (big, big_note, note)
}

#[cfg(test)]
mod tests {
    use mo_space::clean::What;
    use mo_space::song::CompactKind;

    use super::*;

    fn removed(path: &str) -> Removed {
        Removed {
            path: path.to_owned(),
            real: path.to_owned(),
            bytes: 1,
            what: What::Song(CompactKind::Backup),
            when: 0,
            bin_id: None,
            binned: None,
            binned_zone: None,
            stamp: None,
        }
    }

    fn problem(path: &str, why: &str) -> Problem {
        Problem { path: path.to_owned(), why: why.to_owned() }
    }

    #[test]
    fn the_card_says_how_far_remove_has_got() {
        let note = "Each one is checked again just before it goes to the Recycle Bin.";
        let mut tally = Tally::new(true);
        tally.heard(&Ok(()));
        assert_eq!(tally.words(note, 5000), format!("{note} 1 of 5,000 has gone so far."));
        for _ in 0..1233 {
            tally.heard(&Ok(()));
        }
        assert_eq!(tally.words(note, 5000), format!("{note} 1,234 of 5,000 have gone so far."));
        // A program using it: it stays. Windows guarding it: it waits for the step, which asks once at the end.
        tally.heard(&Err(mo_space::clean::IN_USE.to_owned()));
        tally.heard(&Err(NEEDS_ADMIN.to_owned()));
        assert_eq!(
            tally.words(note, 5000),
            format!(
                "{note} 1,234 of 5,000 have gone so far and 1 stays. 1 waits for Windows to ask, once, at the end."
            )
        );
        // The bar counts each one sent, whichever way it went.
        assert!((tally.fraction(5000) - 1236.0 / 5000.0).abs() < 1e-6);
        assert_eq!(Tally::new(true).fraction(0), 0.0);
        // Where Windows can't be asked, what it guards stays.
        let mut no_step = Tally::new(false);
        no_step.heard(&Err(NEEDS_ADMIN.to_owned()));
        no_step.heard(&Err(NEEDS_ADMIN.to_owned()));
        assert_eq!((no_step.went, no_step.waits, no_step.stays), (0, 0, 2));
        assert_eq!(no_step.words(note, 2), format!("{note} 0 of 2 have gone so far and 2 stay."));
    }

    #[test]
    fn the_log_names_the_first_few_and_says_why_the_rest_stayed_once_for_each_reason() {
        let one = ("an old copy", "An old copy");
        let lines = |done: &Done| log_lines(done, one, "old copies");
        let few = Done {
            removed: vec![removed("a"), removed("b")],
            problems: vec![problem("b", NOT_THERE), problem("c", CHANGED), problem("d", CHANGED), problem("e", OTHER)],
            went_with: Vec::new(),
        };
        let said: Vec<(bool, String)> = vec![
            (false, format!("Sent an old copy to the {BIN}: a")),
            (false, format!("Sent an old copy to the {BIN}: b")),
            (true, format!("An old copy went: b: {NOT_THERE}")),
            (true, format!("2 old copies stayed: {CHANGED}")),
            (true, "  c".to_owned()),
            (true, "  d".to_owned()),
            (true, format!("An old copy stayed: e: {OTHER}")),
        ];
        assert_eq!(lines(&few), said);

        // Thousands: the first few by name, the rest counted.
        let many: Vec<String> = (0..LOGGED + 7).map(|n| format!("p{n}")).collect();
        let lots = Done {
            removed: many.iter().map(|p| removed(p)).collect(),
            problems: many.iter().map(|p| problem(&format!("{p}-stayed"), CHANGED)).collect(),
            went_with: Vec::new(),
        };
        let said = lines(&lots);
        assert_eq!(said.len(), LOGGED + 1 + 1 + LOGGED + 1);
        assert_eq!(said.get(LOGGED), Some(&(false, format!("…and sent 7 more old copies to the {BIN}"))));
        assert_eq!(said.get(LOGGED + 1), Some(&(true, format!("{} old copies stayed: {CHANGED}", LOGGED + 7))));
        assert_eq!(said.last(), Some(&(true, "  …and 7 more".to_owned())));

        // A reason for each: the first few reasons, then the rest counted.
        let whys = Done {
            problems: (0..LOGGED + 3).map(|n| problem(&format!("p{n}"), &format!("Reason {n}."))).collect(),
            ..Done::default()
        };
        let said = lines(&whys);
        assert_eq!(said.len(), LOGGED + 1);
        assert_eq!(
            said.last(),
            Some(&(true, format!("…and 3 more old copies stayed or aren't in the {BIN}, for 3 other reasons")))
        );
    }

    /// Something picked, where it is.
    #[derive(Clone, Debug, PartialEq, Eq)]
    struct Picked(&'static str);

    impl Removable for Picked {
        fn path(&self) -> &str {
            self.0
        }
        fn bytes(&self) -> u64 {
            1
        }
        fn can_go(&self) -> bool {
            true
        }
    }

    #[test]
    fn a_crash_part_way_keeps_what_was_noted_going() {
        let picked = [Picked("a"), Picked("b"), Picked("c"), Picked("d"), Picked("e")];
        // a went; b waited for Windows' administrator step and was noted just before it, but is still there (you said
        // no); c, d and e were never sent, and d and e aren't where they were (they may have gone just as Remove
        // stopped).
        let mut waited = removed("b");
        waited.stamp = Some("stamp".to_owned());
        let still_there = |at: &str| ["b", "c"].contains(&at);
        let done = after_crash(&picked, vec![removed("a"), waited], still_there);
        assert_eq!(done.removed, [removed("a")]);
        assert_eq!(
            done.problems,
            [
                problem("b", &crashed_words()),
                problem("c", &crashed_words()),
                problem("d", &crashed_gone_words()),
                problem("e", &crashed_gone_words())
            ]
        );
        assert!(done.went_with.is_empty());
        // What waited and went is kept as it went, with its stamp, for Put back to check.
        let mut went = removed("b");
        went.stamp = Some("stamp".to_owned());
        let done = after_crash(&picked[..2], vec![removed("a"), went.clone()], |_| false);
        assert_eq!((done.removed, done.problems), (vec![removed("a"), went], Vec::new()));
    }

    const NOT_THERE: &str = "It isn't in the Recycle Bin, so it may have been removed for good.";
    const CHANGED: &str = "It changed since you looked, so it stays.";
    const OTHER: &str = "It isn't there any more.";
}
