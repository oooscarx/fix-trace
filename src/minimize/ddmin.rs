use std::collections::BTreeSet;

use async_trait::async_trait;

use crate::{domain::trial::TrialOutcome, error::AppError};

#[async_trait]
pub trait CandidateTester {
    fn normalize(&self, candidate: &BTreeSet<u64>) -> BTreeSet<u64>;

    fn ordered_ids(&self, candidate: &BTreeSet<u64>) -> Vec<u64> {
        candidate.iter().copied().collect()
    }

    async fn outcome(&mut self, candidate: &BTreeSet<u64>) -> Result<TrialOutcome, AppError>;
}

pub async fn ddmin<T: CandidateTester + Send>(
    tester: &mut T,
    initial: BTreeSet<u64>,
) -> Result<BTreeSet<u64>, AppError> {
    let mut current = tester.normalize(&initial);
    let mut granularity = 2_usize;

    while current.len() >= 2 {
        let partitions = partition(&tester.ordered_ids(&current), granularity);
        let mut reduced = false;

        for raw_subset in &partitions {
            let subset = tester.normalize(raw_subset);
            if subset.len() < current.len()
                && tester.outcome(&subset).await? == TrialOutcome::StablePass
            {
                current = subset;
                granularity = granularity.saturating_sub(1).max(2);
                reduced = true;
                break;
            }
        }
        if reduced {
            continue;
        }

        for subset in &partitions {
            let raw_complement: BTreeSet<_> = current.difference(subset).copied().collect();
            let complement = tester.normalize(&raw_complement);
            if complement.len() < current.len()
                && tester.outcome(&complement).await? == TrialOutcome::StablePass
            {
                current = complement;
                granularity = granularity.saturating_sub(1).max(2);
                reduced = true;
                break;
            }
        }
        if reduced {
            continue;
        }

        if granularity >= current.len() {
            break;
        }
        granularity = (granularity * 2).min(current.len());
    }
    Ok(current)
}

fn partition(ids: &[u64], count: usize) -> Vec<BTreeSet<u64>> {
    let count = count.clamp(1, ids.len().max(1));
    let base_size = ids.len() / count;
    let remainder = ids.len() % count;
    let mut result = Vec::with_capacity(count);
    let mut start = 0;
    for index in 0..count {
        let size = base_size + usize::from(index < remainder);
        let end = start + size;
        if start < end {
            result.push(ids[start..end].iter().copied().collect());
        }
        start = end;
    }
    result
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use async_trait::async_trait;

    use super::{CandidateTester, ddmin};
    use crate::{domain::trial::TrialOutcome, error::AppError};

    struct SyntheticTester;

    #[async_trait]
    impl CandidateTester for SyntheticTester {
        fn normalize(&self, candidate: &BTreeSet<u64>) -> BTreeSet<u64> {
            candidate.clone()
        }

        async fn outcome(&mut self, candidate: &BTreeSet<u64>) -> Result<TrialOutcome, AppError> {
            Ok(if candidate.contains(&2) && candidate.contains(&5) {
                TrialOutcome::StablePass
            } else {
                TrialOutcome::StableFail
            })
        }
    }

    #[tokio::test]
    async fn artificial_sequence_reduces_to_expected_one_minimal_set() {
        let minimal = ddmin(&mut SyntheticTester, (1..=7).collect())
            .await
            .expect("ddmin should finish");

        assert_eq!(minimal, BTreeSet::from([2, 5]));
    }
}
