//! The filters' shared parts on screen (`FilterPicks` in studio.slint): a filter as mo-app's `filters` shows it, a
//! short list's choices as chips of their own, a longer one's in a menu, with what's picked.

use std::rc::Rc;

use mo_app::filters::{Filter, Shown};
use slint::{Model, ModelRc, VecModel};

use crate::{FilterChoice, FilterView};

/// One filter's chips and menu lines, kept from one showing to the next. Only the lines that changed are changed, so
/// an open menu keeps its place, and the line under the mouse stays the same line, when a pick or a new look at the
/// songs changes what the lines say. Nothing is made again when nothing changed: a page shows itself often.
#[derive(Default)]
pub struct FilterModels {
    chips: Rc<VecModel<FilterChoice>>,
    more: Rc<VecModel<FilterChoice>>,
}

impl FilterModels {
    /// `filter` for its `FilterPicks`.
    pub fn view(&self, filter: &Filter) -> FilterView {
        update(&self.chips, &filter.chips);
        update(&self.more, &filter.more);
        FilterView {
            chips: ModelRc::from(self.chips.clone()),
            more: ModelRc::from(self.more.clone()),
            more_picked: filter.more_picked.as_str().into(),
        }
    }
}

/// Makes `model` say `list`, changing only the lines that differ (all of them when the number of lines changed).
fn update(model: &VecModel<FilterChoice>, list: &[Shown]) {
    let line = |s: &Shown| FilterChoice { text: s.text.as_str().into(), count: s.count.as_str().into(), on: s.on };
    if model.row_count() != list.len() {
        model.set_vec(list.iter().map(line).collect::<Vec<_>>());
        return;
    }
    for (at, s) in list.iter().enumerate() {
        let same = model
            .row_data(at)
            .is_some_and(|r| r.text == s.text.as_str() && r.count == s.count.as_str() && r.on == s.on);
        if !same {
            model.set_row_data(at, line(s));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(text: &str, on: bool) -> Shown {
        Shown { text: text.into(), count: "1".into(), on }
    }

    #[test]
    fn a_new_showing_changes_only_the_lines_that_changed() {
        let models = FilterModels::default();
        let first = Filter { more: vec![shown("Serum", false), shown("Vital", false)], ..Filter::default() };
        let view = models.view(&first);
        // Picking Vital changes its line in the same list: the menu's model is the one it had.
        let picked = Filter { more: vec![shown("Serum", false), shown("Vital", true)], ..Filter::default() };
        let again = models.view(&picked);
        assert!(again.more == view.more);
        assert!(again.more.row_data(1).is_some_and(|r| r.on));
        assert!(again.more.row_data(0).is_some_and(|r| !r.on));
        // A plugin more: every line again, still in the same model.
        let grown = Filter { more: vec![shown("Diva", false), shown("Serum", false), shown("Vital", true)], ..picked };
        assert_eq!(models.view(&grown).more.row_count(), 3);
        assert!(models.view(&grown).more == view.more);
    }
}
