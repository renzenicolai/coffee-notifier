use base64::{engine::general_purpose::STANDARD, Engine};
use cbc::cipher::block_padding::Pkcs7;
use cbc::cipher::{BlockModeDecrypt, BlockModeEncrypt, KeyIvInit};

type Aes256CbcEnc = cbc::Encryptor<aes::Aes256>;
type Aes256CbcDec = cbc::Decryptor<aes::Aes256>;

// AES-256-CBC, zero IV, PKCS7 padding - matches the Python reference implementation.
const IV: [u8; 16] = [0u8; 16];

pub fn encrypt_dcn(key: &str, plaintext: &str) -> anyhow::Result<String> {
    let enc = Aes256CbcEnc::new_from_slices(key.as_bytes(), &IV)
        .map_err(|e| anyhow::anyhow!("invalid encryption key: {e}"))?;
    let ciphertext = enc.encrypt_padded_vec::<Pkcs7>(plaintext.as_bytes());
    Ok(STANDARD.encode(ciphertext))
}

pub fn decrypt_dcn(key: &str, payload: &[u8]) -> anyhow::Result<String> {
    let buf = STANDARD.decode(payload)?;
    let dec = Aes256CbcDec::new_from_slices(key.as_bytes(), &IV)
        .map_err(|e| anyhow::anyhow!("invalid encryption key: {e}"))?;
    let plaintext = dec
        .decrypt_padded_vec::<Pkcs7>(&buf)
        .map_err(|e| anyhow::anyhow!("decrypt error: {e}"))?;
    Ok(String::from_utf8(plaintext)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Just a fixture for the roundtrip test below, not a real/default key - the legacy
    // encryption key has no default and must be configured explicitly.
    const TEST_KEY: &str = "b14ca5898a4e4142aace2ea2143a2410";

    #[test]
    fn roundtrip() {
        let plaintext = "{\"type\":1,\"location\":\"kitchen\"}";
        let encrypted = encrypt_dcn(TEST_KEY, plaintext).unwrap();
        let decrypted = decrypt_dcn(TEST_KEY, encrypted.as_bytes()).unwrap();
        assert_eq!(plaintext, decrypted);
    }
}
