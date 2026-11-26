use crate::now_secs;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub const MAX_SNIPPETS: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnippetScope {
    Global,
    Server(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SnippetScopeFilter {
    All,
    GlobalOnly,
    ActiveServerOnly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedQuery {
    pub id: String,
    pub name: String,
    pub folder: String,
    pub scope: SnippetScope,
    pub pinned: bool,
    pub sql: String,
    pub updated_at_secs: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SnippetsStore {
    pub version: u8,
    #[serde(default)]
    pub items: Vec<SavedQuery>,
}

impl Default for SnippetsStore {
    fn default() -> Self {
        Self {
            version: 1,
            items: Vec::new(),
        }
    }
}

impl SnippetsStore {
    pub fn upsert(
        &mut self,
        id: Option<String>,
        name: String,
        folder: String,
        scope: SnippetScope,
        pinned: bool,
        sql: String,
    ) -> Result<String, String> {
        let trimmed_name = name.trim().to_string();
        if trimmed_name.is_empty() {
            return Err("Snippet name cannot be empty.".into());
        }
        if sql.trim().is_empty() {
            return Err("Snippet SQL cannot be empty.".into());
        }
        let trimmed_folder = folder.trim().to_string();

        if let Some(target_id) = id.as_deref() {
            if let Some(existing) = self.items.iter_mut().find(|q| q.id == target_id) {
                existing.name = trimmed_name;
                existing.folder = trimmed_folder;
                existing.scope = scope;
                existing.pinned = pinned;
                existing.sql = sql;
                existing.updated_at_secs = now_secs();
                return Ok(existing.id.clone());
            }
        }

        if self.items.len() >= MAX_SNIPPETS {
            return Err(format!("Maximum of {MAX_SNIPPETS} saved queries reached."));
        }

        let new_id = self.unique_id_for(&trimmed_name);
        self.items.push(SavedQuery {
            id: new_id.clone(),
            name: trimmed_name,
            folder: trimmed_folder,
            scope,
            pinned,
            sql,
            updated_at_secs: now_secs(),
        });
        Ok(new_id)
    }

    pub fn delete(&mut self, id: &str) -> bool {
        if let Some(pos) = self.items.iter().position(|q| q.id == id) {
            self.items.remove(pos);
            true
        } else {
            false
        }
    }

    pub fn toggle_pin(&mut self, id: &str) -> bool {
        if let Some(item) = self.items.iter_mut().find(|q| q.id == id) {
            item.pinned = !item.pinned;
            true
        } else {
            false
        }
    }

    pub fn folders(&self) -> Vec<String> {
        let set: BTreeSet<String> = self
            .items
            .iter()
            .map(|q| q.folder.trim())
            .filter(|f| !f.is_empty())
            .map(ToString::to_string)
            .collect();
        set.into_iter().collect()
    }

    pub fn filtered(
        &self,
        search: &str,
        folder: Option<&str>,
        active_server_id: Option<&str>,
        scope_filter: SnippetScopeFilter,
    ) -> Vec<&SavedQuery> {
        let needle = search.trim().to_lowercase();
        let target_folder = folder.map(|f| f.trim().to_lowercase());

        let mut results: Vec<&SavedQuery> = self
            .items
            .iter()
            .filter(|q| match scope_filter {
                SnippetScopeFilter::All => true,
                SnippetScopeFilter::GlobalOnly => matches!(q.scope, SnippetScope::Global),
                SnippetScopeFilter::ActiveServerOnly => match (&q.scope, active_server_id) {
                    (SnippetScope::Server(sid), Some(active)) => sid == active,
                    _ => false,
                },
            })
            .filter(|q| match target_folder.as_deref() {
                Some(expected) => q.folder.trim().to_lowercase() == expected,
                None => true,
            })
            .filter(|q| {
                if needle.is_empty() {
                    return true;
                }
                q.name.to_lowercase().contains(&needle)
                    || q.folder.to_lowercase().contains(&needle)
                    || q.sql.to_lowercase().contains(&needle)
            })
            .collect();

        results.sort_by(|a, b| {
            b.pinned
                .cmp(&a.pinned)
                .then_with(|| b.updated_at_secs.cmp(&a.updated_at_secs))
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
                .then_with(|| a.name.cmp(&b.name))
        });

        results
    }

    fn unique_id_for(&self, name: &str) -> String {
        let slug: String = name
            .chars()
            .filter_map(|c| {
                if c.is_ascii_alphanumeric() {
                    Some(c.to_ascii_lowercase())
                } else if c.is_whitespace() || c == '-' || c == '_' {
                    Some('-')
                } else {
                    None
                }
            })
            .take(24)
            .collect();
        let base = if slug.trim_matches('-').is_empty() {
            "query".to_string()
        } else {
            slug.trim_matches('-').to_string()
        };
        let stamp = now_secs() % 100_000;
        let mut candidate = format!("snip-{base}-{stamp}");
        let mut suffix = 1u32;
        while self.items.iter().any(|q| q.id == candidate) {
            candidate = format!("snip-{base}-{stamp}-{suffix}");
            suffix += 1;
        }
        candidate
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_creates_updates_and_validates() {
        let mut store = SnippetsStore::default();
        assert_eq!(store.version, 1);
        assert!(store.items.is_empty());

        assert!(store
            .upsert(
                None,
                "   ".into(),
                "Ops".into(),
                SnippetScope::Global,
                false,
                "SELECT 1;".into(),
            )
            .is_err());

        assert!(store
            .upsert(
                None,
                "Valid Name".into(),
                "Ops".into(),
                SnippetScope::Global,
                false,
                "   \n\t ".into(),
            )
            .is_err());

        let id = store
            .upsert(
                None,
                "  Active Users  ".into(),
                "  Analytics  ".into(),
                SnippetScope::Global,
                false,
                "SELECT COUNT(*) FROM users;".into(),
            )
            .unwrap();
        assert!(id.starts_with("snip-active-users-"));
        assert_eq!(store.items.len(), 1);
        assert_eq!(store.items[0].name, "Active Users");
        assert_eq!(store.items[0].folder, "Analytics");

        store.items[0].updated_at_secs = 10;

        let updated_id = store
            .upsert(
                Some(id.clone()),
                "Active Users v2".into(),
                "Reporting".into(),
                SnippetScope::Server("srv-1".into()),
                true,
                "SELECT id FROM users WHERE active = 1;".into(),
            )
            .unwrap();
        assert_eq!(updated_id, id);
        assert_eq!(store.items.len(), 1);
        assert_eq!(store.items[0].name, "Active Users v2");
        assert_eq!(store.items[0].folder, "Reporting");
        assert_eq!(store.items[0].scope, SnippetScope::Server("srv-1".into()));
        assert!(store.items[0].pinned);
        assert_eq!(
            store.items[0].sql,
            "SELECT id FROM users WHERE active = 1;"
        );
        assert!(store.items[0].updated_at_secs >= 10);
    }

    #[test]
    fn folders_returns_sorted_deduplicated_non_empty_names() {
        let mut store = SnippetsStore::default();
        store
            .upsert(
                None,
                "Q1".into(),
                "Reporting".into(),
                SnippetScope::Global,
                false,
                "SELECT 1;".into(),
            )
            .unwrap();
        store
            .upsert(
                None,
                "Q2".into(),
                "  Analytics  ".into(),
                SnippetScope::Global,
                false,
                "SELECT 2;".into(),
            )
            .unwrap();
        store
            .upsert(
                None,
                "Q3".into(),
                "Reporting".into(),
                SnippetScope::Global,
                false,
                "SELECT 3;".into(),
            )
            .unwrap();
        store
            .upsert(
                None,
                "Q4".into(),
                "   ".into(),
                SnippetScope::Global,
                false,
                "SELECT 4;".into(),
            )
            .unwrap();
        store
            .upsert(
                None,
                "Q5".into(),
                "".into(),
                SnippetScope::Global,
                false,
                "SELECT 5;".into(),
            )
            .unwrap();

        assert_eq!(store.folders(), vec!["Analytics", "Reporting"]);
    }

    #[test]
    fn filtered_applies_scope_folder_search_and_ordering() {
        let store = SnippetsStore {
            items: vec![
                SavedQuery {
                    id: "1".into(),
                    name: "Older Global".into(),
                    folder: "Analytics".into(),
                    scope: SnippetScope::Global,
                    pinned: false,
                    sql: "SELECT * FROM events;".into(),
                    updated_at_secs: 100,
                },
                SavedQuery {
                    id: "2".into(),
                    name: "Pinned Prod".into(),
                    folder: "Ops".into(),
                    scope: SnippetScope::Server("srv-prod".into()),
                    pinned: true,
                    sql: "SHOW PROCESSLIST;".into(),
                    updated_at_secs: 50,
                },
                SavedQuery {
                    id: "3".into(),
                    name: "Newer Global".into(),
                    folder: "Analytics".into(),
                    scope: SnippetScope::Global,
                    pinned: false,
                    sql: "SELECT COUNT(*) FROM orders;".into(),
                    updated_at_secs: 300,
                },
                SavedQuery {
                    id: "4".into(),
                    name: "Staging Check".into(),
                    folder: "Ops".into(),
                    scope: SnippetScope::Server("srv-staging".into()),
                    pinned: false,
                    sql: "SELECT VERSION();".into(),
                    updated_at_secs: 200,
                },
                SavedQuery {
                    id: "5".into(),
                    name: "Beta Tie".into(),
                    folder: "Analytics".into(),
                    scope: SnippetScope::Global,
                    pinned: true,
                    sql: "SELECT 2;".into(),
                    updated_at_secs: 500,
                },
                SavedQuery {
                    id: "6".into(),
                    name: "Alpha Tie".into(),
                    folder: "Analytics".into(),
                    scope: SnippetScope::Global,
                    pinned: true,
                    sql: "SELECT 1;".into(),
                    updated_at_secs: 500,
                },
            ],
            ..Default::default()
        };

        let all_ids: Vec<&str> = store
            .filtered("", None, Some("srv-prod"), SnippetScopeFilter::All)
            .into_iter()
            .map(|q| q.id.as_str())
            .collect();
        assert_eq!(all_ids, vec!["6", "5", "2", "3", "4", "1"]);

        let global_ids: Vec<&str> = store
            .filtered("", None, Some("srv-prod"), SnippetScopeFilter::GlobalOnly)
            .into_iter()
            .map(|q| q.id.as_str())
            .collect();
        assert_eq!(global_ids, vec!["6", "5", "3", "1"]);

        let prod_ids: Vec<&str> = store
            .filtered(
                "",
                None,
                Some("srv-prod"),
                SnippetScopeFilter::ActiveServerOnly,
            )
            .into_iter()
            .map(|q| q.id.as_str())
            .collect();
        assert_eq!(prod_ids, vec!["2"]);

        let no_active = store.filtered("", None, None, SnippetScopeFilter::ActiveServerOnly);
        assert!(no_active.is_empty());

        let folder_ids: Vec<&str> = store
            .filtered("", Some("  ops "), None, SnippetScopeFilter::All)
            .into_iter()
            .map(|q| q.id.as_str())
            .collect();
        assert_eq!(folder_ids, vec!["2", "4"]);

        let by_sql: Vec<&str> = store
            .filtered("processlist", None, None, SnippetScopeFilter::All)
            .into_iter()
            .map(|q| q.id.as_str())
            .collect();
        assert_eq!(by_sql, vec!["2"]);

        let by_folder: Vec<&str> = store
            .filtered("ANALYT", None, None, SnippetScopeFilter::All)
            .into_iter()
            .map(|q| q.id.as_str())
            .collect();
        assert_eq!(by_folder, vec!["6", "5", "3", "1"]);

        let by_name: Vec<&str> = store
            .filtered("staging", None, None, SnippetScopeFilter::All)
            .into_iter()
            .map(|q| q.id.as_str())
            .collect();
        assert_eq!(by_name, vec!["4"]);
    }

    #[test]
    fn toggle_pin_and_delete_work() {
        let mut store = SnippetsStore::default();
        let id = store
            .upsert(
                None,
                "Check Health".into(),
                "Ops".into(),
                SnippetScope::Global,
                false,
                "SELECT 1;".into(),
            )
            .unwrap();

        assert!(!store.items[0].pinned);
        assert!(store.toggle_pin(&id));
        assert!(store.items[0].pinned);
        assert!(store.toggle_pin(&id));
        assert!(!store.items[0].pinned);
        assert!(!store.toggle_pin("missing-id"));

        assert!(store.delete(&id));
        assert!(store.items.is_empty());
        assert!(!store.delete(&id));
    }

    #[test]
    fn enforces_max_snippets_cap() {
        let mut store = SnippetsStore::default();
        for idx in 0..MAX_SNIPPETS {
            store.items.push(SavedQuery {
                id: format!("snip-{idx}"),
                name: format!("Query {idx}"),
                folder: String::new(),
                scope: SnippetScope::Global,
                pinned: false,
                sql: "SELECT 1;".into(),
                updated_at_secs: idx as u64,
            });
        }
        assert!(store
            .upsert(
                None,
                "Overflow".into(),
                String::new(),
                SnippetScope::Global,
                false,
                "SELECT 1;".into(),
            )
            .is_err());
        assert!(store
            .upsert(
                Some("snip-0".into()),
                "Updated Existing".into(),
                String::new(),
                SnippetScope::Global,
                true,
                "SELECT 2;".into(),
            )
            .is_ok());
    }
}
