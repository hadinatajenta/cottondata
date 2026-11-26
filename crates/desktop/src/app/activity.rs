use std::collections::HashSet;

#[derive(Debug, Default, Clone)]
pub(super) struct SlotActivity {
    pub(super) loading_schemas: HashSet<String>,
    pub(super) loading_table_detail: Option<(String, String)>,
    pub(super) is_refreshing_monitor: bool,
    pub(super) is_session_command_pending: bool,
}

impl SlotActivity {
    pub(super) fn reset(&mut self) {
        *self = Self::default();
    }

    pub(super) fn is_loading_table_detail(&self, schema: &str, table: &str) -> bool {
        self.loading_table_detail
            .as_ref()
            .is_some_and(|(pending_schema, pending_table)| pending_schema == schema && pending_table == table)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_clears_every_pending_operation() {
        let mut activity = SlotActivity {
            loading_schemas: HashSet::from(["app".to_string()]),
            loading_table_detail: Some(("app".into(), "users".into())),
            is_refreshing_monitor: true,
            is_session_command_pending: true,
        };
        activity.reset();
        assert!(activity.loading_schemas.is_empty());
        assert!(activity.loading_table_detail.is_none());
        assert!(!activity.is_refreshing_monitor);
        assert!(!activity.is_session_command_pending);
    }

    #[test]
    fn table_detail_match_requires_same_schema_and_table() {
        let activity = SlotActivity {
            loading_table_detail: Some(("app".into(), "users".into())),
            ..Default::default()
        };
        assert!(activity.is_loading_table_detail("app", "users"));
        assert!(!activity.is_loading_table_detail("app", "orders"));
        assert!(!activity.is_loading_table_detail("other", "users"));
    }
}
