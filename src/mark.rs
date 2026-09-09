//! Pure helpers for dired-style row marks (no Docker / TUI).

use crate::docker::Item;
use std::collections::HashSet;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub id: String,
    pub label: String,
}

pub fn toggle_mark(marked: &mut HashSet<String>, id: &str) {
    if !marked.remove(id) {
        marked.insert(id.to_string());
    }
}

pub fn mark_ids(marked: &mut HashSet<String>, ids: impl IntoIterator<Item = String>) {
    marked.extend(ids);
}

pub fn invert_marks(marked: &mut HashSet<String>, ids: impl IntoIterator<Item = String>) {
    for id in ids {
        toggle_mark(marked, &id);
    }
}

pub fn retain_existing(marked: &mut HashSet<String>, items: &[Item]) {
    marked.retain(|id| items.iter().any(|it| !it.header && it.id == *id));
}

/// Ids of non-header items in `items` at the given indices (visible rows).
pub fn ids_at(items: &[Item], indices: &[usize]) -> Vec<String> {
    indices
        .iter()
        .filter_map(|&i| items.get(i))
        .filter(|it| !it.header)
        .map(|it| it.id.clone())
        .collect()
}

/// Resolve batch targets: marked ∩ current non-header items (stable order = items order).
pub fn resolve_targets(marked: &HashSet<String>, items: &[Item]) -> Vec<Target> {
    items
        .iter()
        .filter(|it| !it.header && marked.contains(&it.id))
        .map(|it| Target {
            id: it.id.clone(),
            label: it.name.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::docker::Item;

    fn item(id: &str, name: &str, header: bool) -> Item {
        Item {
            id: id.into(),
            name: name.into(),
            cells: vec![name.into()],
            header,
        }
    }

    #[test]
    fn toggle_inserts_then_removes() {
        let mut m = HashSet::new();
        toggle_mark(&mut m, "a");
        assert!(m.contains("a"));
        toggle_mark(&mut m, "a");
        assert!(!m.contains("a"));
    }

    #[test]
    fn mark_ids_inserts_all() {
        let mut m = HashSet::new();
        mark_ids(&mut m, ["a".into(), "b".into()]);
        assert_eq!(m.len(), 2);
    }

    #[test]
    fn invert_flips_membership() {
        let mut m = HashSet::from(["a".into()]);
        invert_marks(&mut m, ["a".into(), "b".into()]);
        assert!(!m.contains("a"));
        assert!(m.contains("b"));
    }

    #[test]
    fn retain_drops_missing_and_skips_headers() {
        let mut m = HashSet::from(["gone".into(), "keep".into(), "hdr".into()]);
        let items = vec![
            item("hdr", "Images", true),
            item("keep", "nginx", false),
        ];
        retain_existing(&mut m, &items);
        assert_eq!(m, HashSet::from(["keep".into()]));
    }

    #[test]
    fn ids_at_skips_headers() {
        let items = vec![item("h", "H", true), item("a", "A", false)];
        assert_eq!(ids_at(&items, &[0, 1]), vec!["a".to_string()]);
    }

    #[test]
    fn resolve_targets_intersection_in_item_order() {
        let marked = HashSet::from(["b".into(), "a".into(), "missing".into()]);
        let items = vec![item("a", "A", false), item("b", "B", false)];
        let t = resolve_targets(&marked, &items);
        assert_eq!(
            t,
            vec![
                Target {
                    id: "a".into(),
                    label: "A".into()
                },
                Target {
                    id: "b".into(),
                    label: "B".into()
                },
            ]
        );
    }
}
