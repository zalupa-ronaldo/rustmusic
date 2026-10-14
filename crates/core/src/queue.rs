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
}
