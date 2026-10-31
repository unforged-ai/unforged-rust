use serde_json::Value;

/// One page of a list, and the cursor for the next one.
#[derive(Debug, Clone, PartialEq)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// Pass this back to get the next page; `None` on the last one.
    pub next_cursor: Option<String>,
}

impl<T> Page<T> {
    pub(crate) fn from_json(v: &Value, key: &str, f: impl Fn(&Value) -> T) -> Self {
        Self {
            items: crate::json::arr(v, key).iter().map(f).collect(),
            next_cursor: crate::json::s(v, "next_cursor").filter(|c| !c.is_empty()),
        }
    }
}

impl<T> IntoIterator for Page<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<T>;

    fn into_iter(self) -> Self::IntoIter {
        self.items.into_iter()
    }
}
