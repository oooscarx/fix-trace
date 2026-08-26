use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{domain::trial::Trial, replay::oracle::OracleSpec};

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct TrialCacheKey {
    pub baseline_hash: String,
    pub oracle_hash: String,
    pub action_ids: Vec<u64>,
    pub repetitions: u32,
}

impl TrialCacheKey {
    pub fn new(
        baseline_hash: &str,
        oracle: &OracleSpec,
        action_ids: Vec<u64>,
        repetitions: u32,
    ) -> Self {
        Self {
            baseline_hash: baseline_hash.to_owned(),
            oracle_hash: hash_oracle(oracle),
            action_ids,
            repetitions,
        }
    }
}

#[derive(Default)]
pub struct TrialCache {
    entries: BTreeMap<TrialCacheKey, Trial>,
}

impl TrialCache {
    pub fn get(&self, key: &TrialCacheKey) -> Option<&Trial> {
        self.entries.get(key)
    }

    pub fn insert(&mut self, key: TrialCacheKey, trial: Trial) {
        self.entries.insert(key, trial);
    }
}

fn hash_oracle(oracle: &OracleSpec) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"fixtrace-oracle-v1\0");
    hasher.update((oracle.command.len() as u64).to_le_bytes());
    hasher.update(oracle.command.as_bytes());
    hasher.update(oracle.timeout_ms.to_le_bytes());
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::TrialCacheKey;
    use crate::replay::oracle::OracleSpec;

    #[test]
    fn cache_key_is_stable_and_sensitive_to_required_fields() {
        let oracle = OracleSpec {
            command: "cargo test".to_owned(),
            timeout_ms: 1000,
        };
        let first = TrialCacheKey::new("baseline", &oracle, vec![1, 3], 3);
        let second = TrialCacheKey::new("baseline", &oracle, vec![1, 3], 3);
        let changed = TrialCacheKey::new("baseline", &oracle, vec![1], 3);

        assert_eq!(first, second);
        assert_ne!(first, changed);
    }
}
