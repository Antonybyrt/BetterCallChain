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

    /// Builds a Validator with a deterministic key derived from `seed` and the given stake.
    fn make_validator(seed: u8, stake: u64) -> Validator {
        let signing_key = SigningKey::from_bytes(&[seed; 32]);
        let pubkey = signing_key.verifying_key();
        let address = crate::types::address::Address::from_pubkey_bytes(pubkey.as_bytes());
        Validator { address, pubkey, stake, active_since: 0 }
    }

    /// Election with the same inputs must always return the same validator.
    #[test]
    fn election_is_deterministic() {
        let validators = vec![make_validator(1, 100), make_validator(2, 200)];
        let prev_hash = [0u8; 32];
        let slot = 42;
        let first = elect_proposer(slot, &prev_hash, &validators).unwrap().address.clone();
        let second = elect_proposer(slot, &prev_hash, &validators).unwrap().address.clone();
        assert_eq!(first, second);
    }

    /// A validator with all the stake must always be elected.
    #[test]
    fn sole_validator_always_elected() {
        let validators = vec![make_validator(1, 1000)];
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

    /// A validator with 3× the stake should win ~3× more often over many elections.
    /// Tolerance: ±15% of the expected ratio.
    #[test]
    fn weighted_distribution() {
        let v_low = make_validator(1, 1_000);
        let v_high = make_validator(2, 3_000);
        let validators = vec![v_low, v_high];

        let mut counts = [0u64; 2];
        let trials = 10_000u64;

        for slot in 0..trials {
            let prev_hash = (slot as u64).to_le_bytes();
            let mut seed = [0u8; 32];
            seed[..8].copy_from_slice(&prev_hash);
            let elected = elect_proposer(slot, &seed, &validators).unwrap();
            if elected.address == validators[0].address {
                counts[0] += 1;
            } else {
                counts[1] += 1;
            }
        }

        // v_high has 3× stake → should win ~75% of the time.
        let ratio = counts[1] as f64 / trials as f64;
        assert!(
            (0.60..=0.90).contains(&ratio),
            "expected ~0.75, got {ratio:.3}"
        );
    }
}
