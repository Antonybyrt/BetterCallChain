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
    let pick = u64::from_le_bytes(seed[..8].try_into().expect("seed is 32 bytes")) % total_stake;

    // Weighted linear scan: each validator occupies a segment proportional to its stake.
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

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::SigningKey;
    use rand::rngs::OsRng;

    /// Builds a minimal Validator with the given stake for testing.
    fn make_validator(stake: u64) -> Validator {
        let signing_key = SigningKey::generate(&mut OsRng);
        let pubkey = signing_key.verifying_key();
        let address = crate::types::address::Address::from_pubkey_bytes(pubkey.as_bytes());
        Validator { address, pubkey, stake, active_since: 0 }
    }

    /// Election with the same inputs must always return the same validator.
    #[test]
    fn election_is_deterministic() {
        let validators = vec![make_validator(100), make_validator(200)];
        let prev_hash = [0u8; 32];
        let slot = 42;
        let first = elect_proposer(slot, &prev_hash, &validators).unwrap().address.clone();
        let second = elect_proposer(slot, &prev_hash, &validators).unwrap().address.clone();
        assert_eq!(first, second);
    }

    /// A validator with all the stake must always be elected.
    #[test]
    fn sole_validator_always_elected() {
        let v = make_validator(1000);
        let validators = vec![v];
        let prev_hash = [1u8; 32];
        for slot in 0..10 {
            assert!(elect_proposer(slot, &prev_hash, &validators).is_some());
        }
    }

    /// Returns None when no validators are registered.
    #[test]
    fn empty_set_returns_none() {
        assert!(elect_proposer(0, &[0u8; 32], &[]).is_none());
    }
}
