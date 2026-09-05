//! Sheets: content that is read rather than acted on.
//!
//! A [`FactSheet`] describes one thing — a file, a server, a device — as a
//! title over label/value pairs. A [`ShortcutSheet`] is the help page every
//! keyboard-first app owes its user: the bindings by group, searchable.

use gpui::{App, IntoElement, ParentElement, RenderOnce, SharedString, Styled, Window, div, px};

use crate::ActiveTheme as _;

/// A title and the facts under it, each a `label  value` line at caption
/// size. Sized by the column it sits in.
#[derive(IntoElement)]
pub struct FactSheet {
    title: Option<SharedString>,
    facts: Vec<(SharedString, SharedString)>,
}

impl FactSheet {
    pub fn new() -> Self {
        Self {
            title: None,
            facts: Vec::new(),
        }
    }

    pub fn title(mut self, title: impl Into<SharedString>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn fact(mut self, label: impl Into<SharedString>, value: impl Into<SharedString>) -> Self {
        self.facts.push((label.into(), value.into()));
        self
    }

    pub fn facts(
        mut self,
        facts: impl IntoIterator<Item = (impl Into<SharedString>, impl Into<SharedString>)>,
    ) -> Self {
        self.facts
            .extend(facts.into_iter().map(|(l, v)| (l.into(), v.into())));
        self
    }
}

impl Default for FactSheet {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderOnce for FactSheet {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let (subtitle, caption, dim, bright, gap, small_gap) = (
            theme.type_scale().subtitle(),
            theme.type_scale().caption(),
            theme.dim_foreground(),
            theme.bright_foreground(),
            theme.space().md(),
            theme.space().xs(),
        );
        div()
            .flex()
            .flex_col()
            .gap(px(small_gap))
            .min_h(px(0.))
            .children(self.title.map(|title| {
                div()
                    .text_size(px(subtitle))
                    .text_color(bright)
                    .child(title)
            }))
            .children(self.facts.into_iter().map(|(label, value)| {
                div()
                    .flex()
                    .flex_row()
                    .justify_between()
                    .gap(px(gap))
                    .text_size(px(caption))
                    .child(div().text_color(dim).child(label))
                    .child(div().child(value))
            }))
    }
}

/// One titled group of a [`ShortcutSheet`]: `(keys, what they do)` pairs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutGroup {
    pub title: SharedString,
    pub entries: Vec<(SharedString, SharedString)>,
}

impl ShortcutGroup {
    pub fn new(
        title: impl Into<SharedString>,
        entries: impl IntoIterator<Item = (impl Into<SharedString>, impl Into<SharedString>)>,
    ) -> Self {
        Self {
            title: title.into(),
            entries: entries
                .into_iter()
                .map(|(keys, action)| (keys.into(), action.into()))
                .collect(),
        }
    }
}

/// The groups that survive `query`: a group whose title matches keeps every
/// entry; any other keeps only the entries whose keys or description do.
/// An empty query keeps everything.
pub fn filter_shortcuts(groups: &[ShortcutGroup], query: &str) -> Vec<ShortcutGroup> {
    let query = query.trim().to_lowercase();
    groups
        .iter()
        .filter_map(|group| {
            let kept: Vec<_> = if query.is_empty() || group.title.to_lowercase().contains(&query) {
                group.entries.clone()
            } else {
                group
                    .entries
                    .iter()
                    .filter(|(keys, action)| {
                        keys.to_lowercase().contains(&query)
                            || action.to_lowercase().contains(&query)
                    })
                    .cloned()
                    .collect()
            };
            (!kept.is_empty()).then(|| ShortcutGroup {
                title: group.title.clone(),
                entries: kept,
            })
        })
        .collect()
}

/// The shortcut sheet: every binding by group, filtered by a query. Put it
/// in a large [`crate::Modal`] under a text input, and the input's value is
/// the query.
#[derive(IntoElement)]
pub struct ShortcutSheet {
    groups: Vec<ShortcutGroup>,
    query: String,
}

impl ShortcutSheet {
    pub fn new(groups: impl IntoIterator<Item = ShortcutGroup>) -> Self {
        Self {
            groups: groups.into_iter().collect(),
            query: String::new(),
        }
    }

    /// From a static table — the shape a `const` of bindings takes.
    pub fn from_table(table: &[(&str, &[(&str, &str)])]) -> Self {
        Self::new(table.iter().map(|(title, entries)| {
            ShortcutGroup::new(*title, entries.iter().map(|(k, a)| (*k, *a)))
        }))
    }

    pub fn query(mut self, query: impl Into<String>) -> Self {
        self.query = query.into();
        self
    }
}

impl RenderOnce for ShortcutSheet {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.theme();
        let (caption, body, dim, bright) = (
            theme.type_scale().caption(),
            theme.type_scale().body(),
            theme.dim_foreground(),
            theme.bright_foreground(),
        );
        let (gap, row_gap, col_gap) = (theme.space().md(), theme.space().xs(), theme.space().xl());

        let groups = filter_shortcuts(&self.groups, &self.query);
        if groups.is_empty() {
            return div()
                .text_size(px(caption))
                .text_color(dim)
                .child("no shortcut matches")
                .into_any_element();
        }
        div()
            .flex()
            .flex_col()
            .gap(px(gap))
            .children(groups.into_iter().map(|group| {
                div()
                    .flex()
                    .flex_col()
                    .gap(px(row_gap))
                    .child(
                        div()
                            .text_size(px(caption))
                            .text_color(dim)
                            .child(group.title.to_uppercase()),
                    )
                    .children(group.entries.into_iter().map(|(keys, action)| {
                        div()
                            .flex()
                            .flex_row()
                            .justify_between()
                            .gap(px(col_gap))
                            .text_size(px(body))
                            .child(div().text_color(bright).child(keys))
                            .child(div().text_color(dim).child(action))
                    }))
            }))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> Vec<ShortcutGroup> {
        vec![
            ShortcutGroup::new("Move", [("j / k", "down / up"), ("g / G", "first / last")]),
            ShortcutGroup::new("Act", [("\u{23ce}", "open"), ("d", "delete")]),
        ]
    }

    #[test]
    fn an_empty_query_keeps_everything() {
        assert_eq!(filter_shortcuts(&table(), "  "), table());
    }

    #[test]
    fn a_matching_title_keeps_the_whole_group() {
        let kept = filter_shortcuts(&table(), "mov");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].entries.len(), 2);
    }

    #[test]
    fn otherwise_only_matching_entries_survive() {
        let kept = filter_shortcuts(&table(), "DEL");
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].title, "Act");
        assert_eq!(kept[0].entries, vec![("d".into(), "delete".into())]);
    }

    #[test]
    fn nothing_matching_is_nothing() {
        assert!(filter_shortcuts(&table(), "zzz").is_empty());
    }
}
