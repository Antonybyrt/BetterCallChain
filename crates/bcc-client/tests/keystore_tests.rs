use bcc_client::{error::ClientError, keystore::KeystoreFile};
use bcc_core::types::address::Address;
use tempfile::tempdir;

#[test]
fn round_trip_correct_passphrase() {
    let dir  = tempdir().unwrap();
    let path = dir.path().join("keystore.json");

    let address = KeystoreFile::create(&path, "correct-horse-battery").unwrap();
    let key     = KeystoreFile::load_and_decrypt(&path, "correct-horse-battery").unwrap();
    let derived = Address::from_pubkey_bytes(key.verifying_key().as_bytes());

    assert_eq!(address.to_string(), derived.to_string());
}

#[test]
fn wrong_passphrase_returns_error() {
    let dir  = tempdir().unwrap();
    let path = dir.path().join("keystore.json");

    KeystoreFile::create(&path, "correct").unwrap();
    let result = KeystoreFile::load_and_decrypt(&path, "wrong");

    assert!(matches!(result, Err(ClientError::WrongPassphrase)));
}

#[test]
fn read_address_without_decryption() {
    let dir  = tempdir().unwrap();
    let path = dir.path().join("keystore.json");

    let created = KeystoreFile::create(&path, "pass").unwrap();
    let read    = KeystoreFile::read_address(&path).unwrap();

    assert_eq!(created.to_string(), read.to_string());
}

#[test]
fn two_keystores_produce_different_addresses() {
    let dir = tempdir().unwrap();
    let p1  = dir.path().join("ks1.json");
    let p2  = dir.path().join("ks2.json");

    let a1 = KeystoreFile::create(&p1, "pass").unwrap();
    let a2 = KeystoreFile::create(&p2, "pass").unwrap();

    assert_ne!(a1.to_string(), a2.to_string());
}
