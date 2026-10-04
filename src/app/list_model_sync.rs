use std::collections::HashSet;
use std::hash::Hash;

use gtk::prelude::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum ListModelChange<T> {
    Remove { position: usize },
    Insert { position: usize, item: T },
    Replace { position: usize, item: T },
}

pub(super) fn boxed_list_model_entries<T>(model: &gtk::gio::ListStore) -> Vec<T>
where
    T: Clone + 'static,
{
    (0..model.n_items())
        .map(|position| {
            let object = model
                .item(position)
                .and_downcast::<glib::BoxedAnyObject>()
                .expect("boxed list model must contain boxed entries");
            object.borrow::<T>().clone()
        })
        .collect()
}

pub(super) fn apply_boxed_list_model_changes<T>(
    model: &gtk::gio::ListStore,
    changes: Vec<ListModelChange<T>>,
) where
    T: 'static,
{
    for change in changes {
        match change {
            ListModelChange::Remove { position } => model.remove(position as u32),
            ListModelChange::Insert { position, item } => {
                model.insert(position as u32, &glib::BoxedAnyObject::new(item));
            }
            ListModelChange::Replace { position, item } => {
                model.splice(position as u32, 1, &[glib::BoxedAnyObject::new(item)]);
            }
        }
    }
}

pub(super) fn plan_list_model_changes<T, K>(
    current: &[T],
    desired: &[T],
    identity: impl Fn(&T) -> K,
) -> Vec<ListModelChange<T>>
where
    T: Clone + Eq,
    K: Clone + Eq + Hash,
{
    let desired_identities = desired.iter().map(&identity).collect::<HashSet<_>>();
    let mut working = current.to_vec();
    let mut changes = Vec::new();

    for position in (0..working.len()).rev() {
        if !desired_identities.contains(&identity(&working[position])) {
            working.remove(position);
            changes.push(ListModelChange::Remove { position });
        }
    }

    for (position, desired_item) in desired.iter().enumerate() {
        let desired_identity = identity(desired_item);
        if working
            .get(position)
            .is_some_and(|item| identity(item) == desired_identity)
        {
            if working[position] != *desired_item {
                working[position] = desired_item.clone();
                changes.push(ListModelChange::Replace {
                    position,
                    item: desired_item.clone(),
                });
            }
            continue;
        }

        if let Some(old_position) = working
            .iter()
            .enumerate()
            .skip(position.saturating_add(1))
            .find_map(|(index, item)| (identity(item) == desired_identity).then_some(index))
        {
            working.remove(old_position);
            changes.push(ListModelChange::Remove {
                position: old_position,
            });
        }
        working.insert(position, desired_item.clone());
        changes.push(ListModelChange::Insert {
            position,
            item: desired_item.clone(),
        });
    }

    debug_assert!(working == desired);
    changes
}

#[cfg(test)]
mod tests {
    use super::{
        ListModelChange, apply_boxed_list_model_changes, boxed_list_model_entries,
        plan_list_model_changes,
    };

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct Entry {
        identity: u32,
        value: u32,
    }

    fn entry(identity: u32, value: u32) -> Entry {
        Entry { identity, value }
    }

    fn changes(current: &[Entry], desired: &[Entry]) -> Vec<ListModelChange<Entry>> {
        plan_list_model_changes(current, desired, |entry| entry.identity)
    }

    fn apply(current: &[Entry], changes: &[ListModelChange<Entry>]) -> Vec<Entry> {
        let mut result = current.to_vec();
        for change in changes {
            match change {
                ListModelChange::Remove { position } => {
                    result.remove(*position);
                }
                ListModelChange::Insert { position, item } => {
                    result.insert(*position, item.clone());
                }
                ListModelChange::Replace { position, item } => {
                    result[*position] = item.clone();
                }
            }
        }
        result
    }

    #[test]
    fn boxed_list_model_helpers_read_and_apply_incremental_changes() {
        let model = gtk::gio::ListStore::new::<glib::BoxedAnyObject>();
        model.append(&glib::BoxedAnyObject::new(entry(1, 10)));
        model.append(&glib::BoxedAnyObject::new(entry(2, 20)));

        assert_eq!(
            boxed_list_model_entries::<Entry>(&model),
            [entry(1, 10), entry(2, 20)]
        );

        apply_boxed_list_model_changes(
            &model,
            vec![
                ListModelChange::Remove { position: 0 },
                ListModelChange::Insert {
                    position: 1,
                    item: entry(3, 30),
                },
                ListModelChange::Replace {
                    position: 0,
                    item: entry(2, 21),
                },
            ],
        );

        assert_eq!(
            boxed_list_model_entries::<Entry>(&model),
            [entry(2, 21), entry(3, 30)]
        );
    }

    #[test]
    fn identical_history_entries_have_no_changes() {
        let entries = [entry(1, 10), entry(2, 20)];
        assert!(changes(&entries, &entries).is_empty());
    }

    #[test]
    fn history_deletion_removes_only_the_target_and_never_clears_the_model() {
        let current = [entry(1, 10), entry(2, 20), entry(3, 30)];
        let desired = [entry(1, 10), entry(3, 30)];
        let planned = changes(&current, &desired);
        assert_eq!(planned, [ListModelChange::Remove { position: 1 }]);
        assert_eq!(apply(&current, &planned), desired);
    }

    #[test]
    fn history_addition_inserts_only_the_new_entry() {
        let current = [entry(2, 20), entry(3, 30)];
        let desired = [entry(1, 10), entry(2, 20), entry(3, 30)];
        let planned = changes(&current, &desired);
        assert_eq!(
            planned,
            [ListModelChange::Insert {
                position: 0,
                item: entry(1, 10),
            }]
        );
        assert_eq!(apply(&current, &planned), desired);
    }

    #[test]
    fn history_content_update_replaces_only_that_item() {
        let current = [entry(1, 10), entry(2, 20)];
        let desired = [entry(1, 11), entry(2, 20)];
        let planned = changes(&current, &desired);
        assert_eq!(
            planned,
            [ListModelChange::Replace {
                position: 0,
                item: entry(1, 11),
            }]
        );
        assert_eq!(apply(&current, &planned), desired);
    }

    #[test]
    fn history_existing_entry_can_move_to_the_front_and_update() {
        let current = [entry(1, 10), entry(2, 20), entry(3, 30)];
        let desired = [entry(3, 31), entry(1, 10), entry(2, 20)];
        let planned = changes(&current, &desired);
        assert_eq!(apply(&current, &planned), desired);
        assert_eq!(
            planned,
            [
                ListModelChange::Remove { position: 2 },
                ListModelChange::Insert {
                    position: 0,
                    item: entry(3, 31),
                },
            ]
        );
    }

    #[test]
    fn history_multiple_entries_finish_in_the_requested_order() {
        let current = [entry(1, 10), entry(2, 20), entry(3, 30), entry(4, 40)];
        let desired = [entry(4, 41), entry(2, 20), entry(5, 50), entry(1, 10)];
        let planned = changes(&current, &desired);
        assert_eq!(apply(&current, &planned), desired);
    }

    #[test]
    fn favorites_add_remove_and_unchanged_are_incremental() {
        let first = [entry(1, 0), entry(2, 0)];
        assert!(changes(&first, &first).is_empty());

        let added = [entry(3, 0), entry(1, 0), entry(2, 0)];
        let add_changes = changes(&first, &added);
        assert_eq!(apply(&first, &add_changes), added);
        assert_eq!(add_changes.len(), 1);

        let removed = [entry(3, 0), entry(2, 0)];
        let remove_changes = changes(&added, &removed);
        assert_eq!(apply(&added, &remove_changes), removed);
        assert_eq!(remove_changes, [ListModelChange::Remove { position: 1 }]);
    }
}
