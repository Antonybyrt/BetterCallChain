use bcc_core::consensus::pos::elect_proposer;
use bcc_core::types::{address::Address, validator::Validator};
use ed25519_dalek::SigningKey;

fn make_validator(seed: u8, stake: u64) -> Validator {
    let signing_key = SigningKey::from_bytes(&[seed; 32]);
    let pubkey = signing_key.verifying_key();
    let address = Address::from_pubkey_bytes(pubkey.as_bytes());
    Validator { address, pubkey, stake, active_since: 0 }
}

#[test]
fn election_is_deterministic() {
    let validators = vec![make_validator(1, 100), make_validator(2, 200)];
    let prev_hash = [0u8; 32];
    let slot = 42;
    let first  = elect_proposer(slot, &prev_hash, &validators).unwrap().address.clone();
    let second = elect_proposer(slot, &prev_hash, &validators).unwrap().address.clone();
    assert_eq!(first, second);
}

#[test]
fn sole_validator_always_elected() {
    let validators = vec![make_validator(1, 1000)];
    let prev_hash = [1u8; 32];
    for slot in 0..10 {
        assert!(elect_proposer(slot, &prev_hash, &validators).is_some());
    }
}

#[test]
fn empty_set_returns_none() {
    assert!(elect_proposer(0, &[0u8; 32], &[]).is_none());
}

/// A validator with 3× the stake should win ~75% of elections.
/// Tolerance: ±15%.
#[test]
fn weighted_distribution() {
    let v_low  = make_validator(1, 1_000);
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

    let ratio = counts[1] as f64 / trials as f64;
    assert!(
        (0.60..=0.90).contains(&ratio),
        "expected ~0.75, got {ratio:.3}"
    );
}
