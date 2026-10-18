use crate::Item;
use rand::seq::SliceRandom;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Repeat {
    #[default]
    Off,
    All,
    One,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Queue {
    pub items: Vec<Item>,
    pub index: Option<usize>,
    pub repeat: Repeat,
}
impl Queue {
    pub fn replace(&mut self, items: Vec<Item>, index: usize) {
        self.items = items;
        self.index =
            (!self.items.is_empty()).then_some(index.min(self.items.len().saturating_sub(1)));
    }
    pub fn current(&self) -> Option<&Item> {
        self.index.and_then(|i| self.items.get(i))
    }
    /// The track that should be prepared while the current one plays.
    pub fn upcoming(&self) -> Option<&Item> {
        let i = self.index?;
        if self.repeat == Repeat::One || self.items.len() < 2 {
            return None;
        }
        if i + 1 < self.items.len() {
            return self.items.get(i + 1);
        }
        if self.repeat == Repeat::All {
            return self.items.first();
        }
        None
    }
    pub fn advance(&mut self, ended: bool) -> Option<&Item> {
        let i = self.index?;
        if ended && self.repeat == Repeat::One {
            return self.current();
        }
        if i + 1 < self.items.len() {
            self.index = Some(i + 1);
        } else if self.repeat == Repeat::All {
            self.index = Some(0);
        } else {
            self.index = None;
        }
        self.current()
    }
    pub fn previous(&mut self) -> Option<&Item> {
        let i = self.index?;
        self.index = Some(i.saturating_sub(1));
        self.current()
    }
    pub fn shuffle(&mut self) {
        if let Some(i) = self.index {
            self.items.swap(0, i);
            self.index = Some(0);
            self.items[1..].shuffle(&mut rand::rng());
        } else {
            self.items.shuffle(&mut rand::rng());
        }
    }
    pub fn append(&mut self, item: Item) {
        self.items.push(item);
    }
    /// Place `from` at index `to`. The track that is current stays current.
    pub fn reorder(&mut self, from: usize, to: usize) {
        if !reorder_vec(&mut self.items, from, to) {
            return;
        }
        self.index = self.index.map(|index| shifted_index(index, from, to));
    }
    /// Insert a track to play after the current one. An empty queue just gains the track.
    pub fn insert_next(&mut self, item: Item) -> bool {
        if self.items.len() >= 500 {
            return false;
        }
        let at = self
            .index
            .map(|index| index.saturating_add(1))
            .unwrap_or(self.items.len())
            .min(self.items.len());
        self.items.insert(at, item);
        true
    }
}
pub fn reorder_vec<T>(items: &mut Vec<T>, from: usize, to: usize) -> bool {
    if from == to || from >= items.len() || to >= items.len() {
        return false;
    }
    let item = items.remove(from);
    items.insert(to, item);
    true
}
fn shifted_index(index: usize, from: usize, to: usize) -> usize {
    if index == from {
        to
    } else if from < to && index > from && index <= to {
        index - 1
    } else if to < from && index >= to && index < from {
        index + 1
    } else {
        index
    }
}
/// Where a playlist row should move on YouTube Music.
/// `before_set_video_id` is the row that must end up immediately after the
/// moved track. `None` means the track moves to the end.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaylistReorder {
    pub set_video_id: String,
    pub before_set_video_id: Option<String>,
}
pub fn playlist_reorder(items: &[Item], from: usize, to: usize) -> Option<PlaylistReorder> {
    if from == to || from >= items.len() || to >= items.len() {
        return None;
    }
    let set_video_id = items[from].set_video_id.clone()?;
    let mut order: Vec<Option<String>> =
        items.iter().map(|item| item.set_video_id.clone()).collect();
    let moved = order.remove(from);
    order.insert(to, moved);
    let before = order
        .get(to + 1)
        .and_then(|id| id.clone())
        .filter(|id| !id.is_empty());
    Some(PlaylistReorder {
        set_video_id,
        before_set_video_id: before,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    fn items() -> Vec<Item> {
        (0..3)
            .map(|i| Item {
                id: i.to_string(),
                ..Default::default()
            })
            .collect()
    }
    #[test]
    fn ends_without_repeat() {
        let mut q = Queue::default();
        q.replace(items(), 2);
        assert!(q.advance(true).is_none());
    }
    #[test]
    fn repeats_one_only_on_end() {
        let mut q = Queue::default();
        q.replace(items(), 0);
        q.repeat = Repeat::One;
        assert_eq!(q.advance(true).unwrap().id, "0");
        assert_eq!(q.advance(false).unwrap().id, "1");
    }
    #[test]
    fn wraps_and_preserves_current_on_shuffle() {
        let mut q = Queue::default();
        q.replace(items(), 2);
        q.repeat = Repeat::All;
        assert_eq!(q.advance(true).unwrap().id, "0");
        q.shuffle();
        assert_eq!(q.current().unwrap().id, "0");
        assert_eq!(q.items.len(), 3);
    }
    #[test]
    fn upcoming_skips_repeat_one_and_wraps() {
        let mut q = Queue::default();
        q.replace(items(), 1);
        assert_eq!(q.upcoming().unwrap().id, "2");
        q.repeat = Repeat::One;
        assert!(q.upcoming().is_none());
        q.repeat = Repeat::All;
        q.index = Some(2);
        assert_eq!(q.upcoming().unwrap().id, "0");
        q.repeat = Repeat::Off;
        assert!(q.upcoming().is_none());
    }
    #[test]
    fn empty_queue_is_safe() {
        let mut q = Queue::default();
        q.replace(vec![], 99);
        q.shuffle();
        assert!(q.previous().is_none());
        assert!(q.advance(true).is_none());
    }
    #[test]
    fn reorder_keeps_the_current_track() {
        let mut q = Queue::default();
        q.replace(items(), 0);
        q.reorder(0, 2);
        assert_eq!(
            q.items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["1", "2", "0"]
        );
        assert_eq!(q.current().unwrap().id, "0");
        q.reorder(2, 0);
        assert_eq!(
            q.items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["0", "1", "2"]
        );
        assert_eq!(q.index, Some(0));
        q.index = Some(1);
        q.reorder(2, 0);
        assert_eq!(
            q.items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["2", "0", "1"]
        );
        assert_eq!(q.index, Some(2));
        assert_eq!(q.current().unwrap().id, "1");
        q.reorder(0, 0);
        assert_eq!(q.items.len(), 3);
    }
    #[test]
    fn insert_next_follows_the_current_track() {
        let mut q = Queue::default();
        q.replace(items(), 1);
        assert!(q.insert_next(Item {
            id: "next".into(),
            ..Default::default()
        }));
        assert_eq!(
            q.items
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["0", "1", "next", "2"]
        );
        assert_eq!(q.current().unwrap().id, "1");
        let mut empty = Queue::default();
        assert!(empty.insert_next(Item {
            id: "only".into(),
            ..Default::default()
        }));
        assert_eq!(empty.items[0].id, "only");
        assert!(empty.index.is_none());
    }
    #[test]
    fn playlist_reorder_names_the_following_row() {
        let rows: Vec<Item> = ["a", "b", "c", "d"]
            .into_iter()
            .map(|id| Item {
                id: id.into(),
                set_video_id: Some(id.into()),
                ..Default::default()
            })
            .collect();
        let middle = playlist_reorder(&rows, 0, 2).unwrap();
        assert_eq!(middle.set_video_id, "a");
        assert_eq!(middle.before_set_video_id.as_deref(), Some("d"));
        let end = playlist_reorder(&rows, 1, 3).unwrap();
        assert_eq!(end.set_video_id, "b");
        assert!(end.before_set_video_id.is_none());
        assert!(playlist_reorder(&rows, 1, 1).is_none());
        let mut unpositioned = rows.clone();
        unpositioned[0].set_video_id = None;
        assert!(playlist_reorder(&unpositioned, 0, 2).is_none());
    }
}
