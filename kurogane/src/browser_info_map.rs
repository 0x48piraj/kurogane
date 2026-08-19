use std::{collections::BTreeMap, ops::ControlFlow};

use crate::browser_registry::BrowserId;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BrowserInfoMapVisitorResult {
    RemoveEntry,
    KeepEntry,
}

pub trait BrowserInfoMapVisitor<K: Copy + Ord, V: Clone> {
    fn on_next_info(
        &self,
        browser_id: BrowserId,
        key: K,
        value: &V,
    ) -> ControlFlow<BrowserInfoMapVisitorResult, BrowserInfoMapVisitorResult>;
}

pub struct BrowserInfoMap<K: Clone + Ord, V: Clone> {
    map: BTreeMap<BrowserId, BTreeMap<K, V>>,
}

impl<K: Clone + Ord, V: Clone> Default for BrowserInfoMap<K, V> {
    fn default() -> Self {
        Self {
            map: BTreeMap::new(),
        }
    }
}

impl<K: Copy + Ord, V: Clone> BrowserInfoMap<K, V> {
    pub fn insert(&mut self, browser_id: BrowserId, key: K, value: V) {
        self.map.entry(browser_id).or_default().insert(key, value);
    }

    pub fn find(
        &mut self,
        browser_id: BrowserId,
        key: K,
        visitor: Option<&dyn BrowserInfoMapVisitor<K, V>>,
    ) -> Option<V> {
        let info_map = self.map.get_mut(&browser_id)?;
        let entry = info_map.get(&key)?;

        if let Some(visitor) = visitor {
            let result = match visitor.on_next_info(browser_id, key, entry) {
                ControlFlow::Break(result) => result,
                ControlFlow::Continue(result) => result,
            };

            if result == BrowserInfoMapVisitorResult::RemoveEntry {
                let entry = info_map.remove(&key);
                if info_map.is_empty() {
                    self.map.remove(&browser_id);
                }
                return entry;
            }
        }

        Some(entry.clone())
    }

    pub fn find_all(&mut self, visitor: &dyn BrowserInfoMapVisitor<K, V>) {
        let browser_ids: Vec<_> = self.map.keys().copied().collect();
        for browser_id in browser_ids {
            let info_map = self
                .map
                .get_mut(&browser_id)
                .expect("missing browser info map");

            let mut keep_going = true;
            let mut removed = vec![];
            let keys: Vec<_> = info_map.keys().copied().collect();
            for key in keys {
                let value = info_map.get(&key).expect("missing value");
                let result = visitor.on_next_info(browser_id, key, value);
                let (stop, result) = match result {
                    ControlFlow::Break(result) => (true, result),
                    ControlFlow::Continue(result) => (false, result),
                };

                if result == BrowserInfoMapVisitorResult::RemoveEntry {
                    removed.push(key);
                }

                if stop {
                    keep_going = false;
                    break;
                }
            }

            for key in removed {
                info_map.remove(&key);
            }

            if info_map.is_empty() {
                self.map.remove(&browser_id);
            }

            if !keep_going {
                break;
            }
        }
    }

    pub fn find_browser_all(
        &mut self,
        browser_id: BrowserId,
        visitor: &dyn BrowserInfoMapVisitor<K, V>,
    ) {
        if self.map.is_empty() {
            return;
        }

        let Some(info_map) = self.map.get_mut(&browser_id) else {
            return;
        };

        let mut removed = vec![];
        let keys: Vec<_> = info_map.keys().copied().collect();
        for key in keys {
            let value = info_map.get(&key).expect("missing value");
            let result = visitor.on_next_info(browser_id, key, value);
            let (stop, result) = match result {
                ControlFlow::Break(result) => (true, result),
                ControlFlow::Continue(result) => (false, result),
            };

            if result == BrowserInfoMapVisitorResult::RemoveEntry {
                removed.push(key);
            }

            if stop {
                break;
            }
        }

        for key in removed {
            info_map.remove(&key);
        }

        if info_map.is_empty() {
            self.map.remove(&browser_id);
        }
    }

    /// Remove a specific (browser_id, key) entry. Returns the removed value if found.
    pub fn remove(&mut self, browser_id: BrowserId, key: K) -> Option<V> {
        let info_map = self.map.get_mut(&browser_id)?;
        let value = info_map.remove(&key);
        if info_map.is_empty() {
            self.map.remove(&browser_id);
        }
        value
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn len(&self) -> usize {
        self.map.values().map(|info_map| info_map.len()).sum()
    }

    pub fn browser_len(&self, browser_id: BrowserId) -> usize {
        self.map
            .get(&browser_id)
            .map_or(0, |info_map| info_map.len())
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }
}
