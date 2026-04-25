use crate::crypto::hash::{sha256d, BlockHash};
use crate::types::validator::Validator;

/// Deterministically elects a block proposer for the given slot.
///
/// The election is weighted by stake: a validator with twice the stake has twice
/// the probability of being elected. All nodes reach the same result without communication,
/// since the only inputs are the previous block hash and the slot number.
///
/// Returns `None` if the validator set is empty or has zero total stake.
pub fn elect_proposer<'a>(
    slot: u64,
    prev_hash: &BlockHash,
    validators: &'a [Validator],
) -> Option<&'a Validator> {
    if validators.is_empty() {
        return None;
    }

    let total_stake: u64 = validators.iter().map(|v| v.stake).sum();
    if total_stake == 0 {
        return None;
    }

    // Seed = sha256d(prev_hash || slot) — deterministic, unpredictable before prev block is known.
    let mut seed_input = Vec::with_capacity(40);
    seed_input.extend_from_slice(prev_hash);
    seed_input.extend_from_slice(&slot.to_le_bytes());
    let seed = sha256d(&seed_input);

    // Use the first 8 bytes of the seed to pick a point in [0, total_stake).
    let bytes = seed[..8].try_into().unwrap();
    let pick = u64::from_le_bytes(bytes) % total_stake;

    // Weighted linear scan: each validator occupies a segment proportional to its stake.
    // Callers must pass validators sorted by address (see ValidatorStore::all_active).
    let mut acc: u64 = 0;
    for validator in validators {
        acc += validator.stake;
        if pick < acc {
            return Some(validator);
        }
    }

    // Unreachable: pick < total_stake and acc reaches total_stake after the full scan.
    None
}
