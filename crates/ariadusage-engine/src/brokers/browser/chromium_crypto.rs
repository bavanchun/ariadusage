use aes::Aes128;
use cbc::cipher::{BlockModeDecrypt, KeyIvInit, block_padding::Pkcs7};
use pbkdf2::pbkdf2_hmac;
use sha1::Sha1;
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

type Aes128CbcDecryptor = cbc::Decryptor<Aes128>;

const SALT: &[u8] = b"saltysalt";
const ITERATIONS: u32 = 1;
const IV: [u8; 16] = [0x20; 16];
const V10_PASSWORD: &[u8] = b"peanuts";
const EMPTY_PASSWORD: &[u8] = b"";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DecryptFailure {
    InvalidCiphertext,
    HostHashMismatch,
    InvalidText,
}

pub(super) fn decrypt_v10(
    encrypted: &[u8],
    host: &str,
    database_version: i64,
) -> Result<Zeroizing<Vec<u8>>, DecryptFailure> {
    decrypt_with_password(encrypted, V10_PASSWORD, host, database_version)
}

pub(super) fn decrypt_v11(
    encrypted: &[u8],
    password: &[u8],
    host: &str,
    database_version: i64,
) -> Result<Zeroizing<Vec<u8>>, DecryptFailure> {
    decrypt_with_password(encrypted, password, host, database_version)
}

pub(super) fn decrypt_empty_password(
    encrypted: &[u8],
    host: &str,
    database_version: i64,
) -> Result<Zeroizing<Vec<u8>>, DecryptFailure> {
    decrypt_with_password(encrypted, EMPTY_PASSWORD, host, database_version)
}

fn derive_key(password: &[u8]) -> Zeroizing<[u8; 16]> {
    let mut key = Zeroizing::new([0_u8; 16]);
    pbkdf2_hmac::<Sha1>(password, SALT, ITERATIONS, &mut key[..]);
    key
}

fn decrypt_with_password(
    encrypted: &[u8],
    password: &[u8],
    host: &str,
    database_version: i64,
) -> Result<Zeroizing<Vec<u8>>, DecryptFailure> {
    let key = derive_key(password);
    let mut plaintext = Zeroizing::new(encrypted.to_vec());
    let plaintext_len = {
        let decryptor = Aes128CbcDecryptor::new_from_slices(&key[..], &IV)
            .map_err(|_| DecryptFailure::InvalidCiphertext)?;
        let decrypted = decryptor
            .decrypt_padded::<Pkcs7>(&mut plaintext)
            .map_err(|_| DecryptFailure::InvalidCiphertext)?;
        decrypted.len()
    };
    plaintext.truncate(plaintext_len);

    if database_version >= 24 {
        let expected = Sha256::digest(host.as_bytes());
        if plaintext.len() < expected.len() || plaintext[..expected.len()] != expected[..] {
            return Err(DecryptFailure::HostHashMismatch);
        }
        plaintext.drain(..expected.len());
    }
    std::str::from_utf8(&plaintext).map_err(|_| DecryptFailure::InvalidText)?;
    Ok(plaintext)
}

#[cfg(feature = "test-hooks")]
pub fn derive_test_key(password: &[u8]) -> [u8; 16] {
    *derive_key(password)
}

#[cfg(feature = "test-hooks")]
pub fn encrypt_test(plaintext: &[u8], password: &[u8]) -> Result<Vec<u8>, DecryptFailure> {
    use cbc::cipher::BlockModeEncrypt;

    let key = derive_key(password);
    let mut buffer = Zeroizing::new(plaintext.to_vec());
    let original_len = buffer.len();
    buffer.resize(original_len + 16, 0);
    let encryptor = cbc::Encryptor::<Aes128>::new_from_slices(&key[..], &IV)
        .map_err(|_| DecryptFailure::InvalidCiphertext)?;
    let encrypted = encryptor
        .encrypt_padded::<Pkcs7>(&mut buffer, original_len)
        .map_err(|_| DecryptFailure::InvalidCiphertext)?;
    Ok(encrypted.to_vec())
}
