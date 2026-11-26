use std::collections::VecDeque;

pub const DEFAULT_GRID_BUDGET_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_INLINE_CELL_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BudgetStats {
    pub used_bytes: usize,
    pub limit_bytes: usize,
    pub tracked_entries: usize,
    pub eviction_count: u64,
}

impl BudgetStats {
    pub fn utilization_pct(&self) -> f32 {
        if self.limit_bytes == 0 {
            return 0.0;
        }
        (self.used_bytes as f32 / self.limit_bytes as f32) * 100.0
    }
}

#[derive(Debug, Clone)]
struct BudgetEntry {
    key: String,
    bytes: usize,
}

#[derive(Debug, Clone)]
pub struct MemoryBudget {
    limit_bytes: usize,
    used_bytes: usize,
    eviction_count: u64,
    lru: VecDeque<BudgetEntry>,
}

impl Default for MemoryBudget {
    fn default() -> Self {
        Self::new(DEFAULT_GRID_BUDGET_BYTES)
    }
}

impl MemoryBudget {
    pub fn new(limit_bytes: usize) -> Self {
        Self {
            limit_bytes: limit_bytes.max(1024 * 1024),
            used_bytes: 0,
            eviction_count: 0,
            lru: VecDeque::new(),
        }
    }

    pub fn record_allocation(&mut self, key: impl Into<String>, bytes: usize) -> Vec<String> {
        let key = key.into();
        self.remove(&key);
        self.lru.push_back(BudgetEntry { key, bytes });
        self.used_bytes = self.used_bytes.saturating_add(bytes);
        self.evict_if_needed()
    }

    pub fn touch(&mut self, key: &str) {
        if let Some(pos) = self.lru.iter().position(|e| e.key == key) {
            if let Some(entry) = self.lru.remove(pos) {
                self.lru.push_back(entry);
            }
        }
    }

    pub fn remove(&mut self, key: &str) -> usize {
        if let Some(pos) = self.lru.iter().position(|e| e.key == key) {
            if let Some(entry) = self.lru.remove(pos) {
                self.used_bytes = self.used_bytes.saturating_sub(entry.bytes);
                return entry.bytes;
            }
        }
        0
    }

    fn evict_if_needed(&mut self) -> Vec<String> {
        let mut evicted = Vec::new();
        while self.used_bytes > self.limit_bytes && self.lru.len() > 1 {
            if let Some(oldest) = self.lru.pop_front() {
                self.used_bytes = self.used_bytes.saturating_sub(oldest.bytes);
                self.eviction_count += 1;
                evicted.push(oldest.key);
            }
        }
        evicted
    }

    pub fn stats(&self) -> BudgetStats {
        BudgetStats {
            used_bytes: self.used_bytes,
            limit_bytes: self.limit_bytes,
            tracked_entries: self.lru.len(),
            eviction_count: self.eviction_count,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_evicts_oldest_pages_beyond_budget() {
        let mut budget = MemoryBudget::new(2 * 1024 * 1024);
        assert!(budget.record_allocation("tab-1", 1024 * 1024).is_empty());
        assert!(budget.record_allocation("tab-2", 1024 * 1024).is_empty());
        budget.touch("tab-1");
        let evicted = budget.record_allocation("tab-3", 1024 * 1024);
        assert_eq!(evicted, vec!["tab-2".to_string()]);
        assert_eq!(budget.stats().used_bytes, 2 * 1024 * 1024);
        assert_eq!(budget.stats().eviction_count, 1);
    }
}
