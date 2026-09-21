use std::collections::HashSet;

use crate::auth::password::{normalize_username, validate_username};

#[derive(Debug, Clone)]
pub struct IssueCleanupPolicy {
    exempt_usernames: HashSet<String>,
    exempt_usernames_json: String,
}

impl Default for IssueCleanupPolicy {
    fn default() -> Self {
        Self::from_normalized_usernames(HashSet::new())
    }
}

impl IssueCleanupPolicy {
    pub fn from_csv(value: Option<&str>) -> (Self, Vec<String>) {
        let mut usernames = HashSet::new();
        let mut ignored = Vec::new();
        for raw in value.unwrap_or_default().split(',') {
            let username = raw.trim();
            if username.is_empty() {
                continue;
            }
            if validate_username(username).is_err() {
                ignored.push(username.to_owned());
                continue;
            }
            usernames.insert(normalize_username(username));
        }
        (Self::from_normalized_usernames(usernames), ignored)
    }

    fn from_normalized_usernames(usernames: HashSet<String>) -> Self {
        let mut sorted = usernames.iter().cloned().collect::<Vec<_>>();
        sorted.sort_unstable();
        let exempt_usernames_json = serde_json::to_string(&sorted).unwrap_or_else(|_| "[]".into());
        Self {
            exempt_usernames: usernames,
            exempt_usernames_json,
        }
    }

    pub fn is_exempt(&self, username: &str) -> bool {
        self.exempt_usernames
            .contains(&normalize_username(username))
    }

    pub fn exempt_usernames_json(&self) -> &str {
        &self.exempt_usernames_json
    }

    pub fn usernames(&self) -> impl Iterator<Item = &str> {
        self.exempt_usernames.iter().map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.exempt_usernames.len()
    }

    pub fn is_empty(&self) -> bool {
        self.exempt_usernames.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::IssueCleanupPolicy;

    #[test]
    fn parses_and_normalizes_exempt_usernames_deterministically() {
        let (policy, ignored) =
            IssueCleanupPolicy::from_csv(Some(" Alice,BOB, alice, invalid name, "));
        assert_eq!(ignored, vec!["invalid name"]);
        assert!(policy.is_exempt("ALIce"));
        assert!(policy.is_exempt("bob"));
        assert_eq!(policy.exempt_usernames_json(), "[\"alice\",\"bob\"]");
    }

    #[test]
    fn empty_values_produce_an_empty_json_array() {
        let (policy, ignored) = IssueCleanupPolicy::from_csv(Some(" , "));
        assert!(ignored.is_empty());
        assert_eq!(policy.len(), 0);
        assert_eq!(policy.exempt_usernames_json(), "[]");
    }
}
