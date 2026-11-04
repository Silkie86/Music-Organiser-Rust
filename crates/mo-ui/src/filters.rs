//! The filters' shared parts on screen (`FilterPicks` in studio.slint): a filter as mo-app's `filters` shows it, a
//! short list's choices as chips of their own, a longer one's in a menu, with what's picked.

use mo_app::filters::{Filter, Shown};
use slint::{ModelRc, VecModel};

use crate::{FilterChoice, FilterView};

fn choices(list: &[Shown]) -> ModelRc<FilterChoice> {
    let list: Vec<FilterChoice> = list
        .iter()
        .map(|s| FilterChoice { text: s.text.as_str().into(), count: s.count.as_str().into(), on: s.on })
        .collect();
    ModelRc::new(VecModel::from(list))
}

/// A filter for its `FilterPicks`.
pub fn view(filter: &Filter) -> FilterView {
    FilterView {
        chips: choices(&filter.chips),
        more: choices(&filter.more),
        more_picked: filter.more_picked.as_str().into(),
    }
}
